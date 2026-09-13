//! `POST /api/audit/browser`: bounded browser diagnostics intake.
//!
//! Status contract (kept compatible with the Python handler and the legacy
//! page, which treats any non-2xx answer as "retry this batch later"):
//! `501` unconfigured, `202 {"ok":true,"accepted":N,"skipped":K,"dropped":B}`
//! (dropped batches still answer `202` so the page does not resend them),
//! `400` malformed body, `413` body over the route limit or more than the
//! event cap, `429` per-client token bucket (`Retry-After`), `503` during
//! shutdown or when every parse slot is busy (`Retry-After: 1`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    Extension, Json,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::{
    audit::{Refusal, Rejection},
    error::ApiError,
    state::AppState,
};

fn retry_after(error: ApiError, seconds: u64) -> Response {
    let mut response = error.into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, seconds.max(1).into());
    response
}

pub async fn browser(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    request: Request,
) -> Result<Response, ApiError> {
    let Some(service) = state.audit.clone() else {
        return Err(ApiError::unavailable(request.uri().path()));
    };
    let client = peer
        .map(|Extension(ConnectInfo(address))| address.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let content_length = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    let admission = match service.admit(client, content_length) {
        Ok(admission) => admission,
        Err(Refusal::Closed) => {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "shutdown",
                "服务正在关闭",
            ));
        }
        Err(Refusal::RateLimited { retry_after: wait }) => {
            return Ok(retry_after(
                ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    "浏览器诊断上报过于频繁",
                ),
                wait.as_secs_f64().ceil() as u64,
            ));
        }
        Err(Refusal::Busy) => {
            return Ok(retry_after(
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "audit_busy",
                    "诊断解析槽位繁忙，请稍后重试",
                ),
                1,
            ));
        }
    };
    if admission.dropped() {
        // Queue saturated: acknowledge without copying the body.
        return Ok(accepted(0, 0, true));
    }
    let limit = service.limits().body_bytes;
    let body = axum::body::to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| {
            service.note_rejected();
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "浏览器诊断请求体过大",
            )
        })?;
    match service.submit(admission, client, &body) {
        Ok(outcome) => Ok(accepted(outcome.accepted, outcome.skipped, outcome.dropped)),
        Err(Rejection::Malformed(message)) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_audit_request",
            message,
        )),
        Err(Rejection::TooManyEvents) => Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_many_events",
            "单次诊断批次事件过多",
        )),
    }
}

fn accepted(accepted: usize, skipped: usize, dropped: bool) -> Response {
    (
        StatusCode::ACCEPTED,
        Json(json!({"ok": true, "accepted": accepted, "skipped": skipped, "dropped": dropped})),
    )
        .into_response()
}
