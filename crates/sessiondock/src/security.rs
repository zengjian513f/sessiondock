//! Loopback Host gate and same-origin API policy. Not authentication.
//! `local_only` (loopback listener) rejects non-loopback Host and any hub
//! protocol/token header, then applies `api_policy`; the node listener runs
//! `api::node_auth` instead of the Host gate and applies the same policy:
//! cross-site and mismatched Origin on `/api/`, oversize URI/body,
//! plus nosniff, same-origin referrer and `no-store` on API responses that
//! omitted cache headers. `Config::public_hosts` (`SESSIONDOCK_PUBLIC_HOSTS`)
//! names the exact authorities an authenticating reverse proxy forwards
//! (`Host $http_host`), which the Host gate then accepts alongside loopback;
//! Origin is still compared against that same authority.
use std::net::IpAddr;

use axum::{
    extract::{Request, State},
    http::{StatusCode, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{error::ApiError, state::AppState};

/// `Host` header or URI authority as sent; both listeners key Origin on it.
fn authority(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()))
}

/// Loopback, or one of the configured public authorities (lower-cased
/// `host` / `host:port`, compared as sent).
pub fn host_allowed(authority: &Authority, public_hosts: &[String]) -> bool {
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        || public_hosts
            .iter()
            .any(|public| public == &authority.as_str().to_ascii_lowercase())
}

pub async fn local_only(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let allowed = authority(&request)
        .and_then(|h| h.parse::<Authority>().ok())
        .is_some_and(|a| host_allowed(&a, &state.public_hosts));
    if !allowed {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "local_only",
            "仅允许本地 loopback Host",
        )
        .into_response();
    }
    // Never accept a protocol/token header as an authentication bypass: hub
    // traffic has its own listener, browsers never send these.
    if request.headers().contains_key("x-agenthub-protocol")
        || request.headers().contains_key("x-agenthub-node-token")
    {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "hub_unsupported",
            "尚不支持旧 Hub 节点协议",
        )
        .into_response();
    }
    api_policy(request, next).await
}

/// Request policy shared by both listeners, after their own gate.
pub async fn api_policy(request: Request, next: Next) -> Response {
    if request.uri().path().starts_with("/api/") {
        if request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|h| h == "cross-site")
        {
            return ApiError::new(StatusCode::FORBIDDEN, "cross_site", "拒绝跨站 API 请求")
                .into_response();
        }
        if let Some(origin) = request.headers().get(header::ORIGIN) {
            let authority = authority(&request).unwrap_or_default();
            let matches = origin.to_str().ok().is_some_and(|origin| {
                ["http", "https"]
                    .iter()
                    .any(|scheme| origin == format!("{scheme}://{authority}"))
            });
            if !matches {
                return ApiError::new(StatusCode::FORBIDDEN, "cross_origin", "拒绝跨源 API 请求")
                    .into_response();
            }
        }
    }
    if request.uri().to_string().len() > 16 * 1024 {
        return ApiError::new(StatusCode::URI_TOO_LONG, "uri_too_long", "请求 URI 过长")
            .into_response();
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<u64>().ok())
        .is_some_and(|n| n > 4 * 1024 * 1024)
    {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "请求体过大",
        )
        .into_response();
    }
    let api_request = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    if !response.headers().contains_key(header::REFERRER_POLICY) {
        response
            .headers_mut()
            .insert(header::REFERRER_POLICY, "same-origin".parse().unwrap());
    }
    if api_request && !response.headers().contains_key(header::CACHE_CONTROL) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    }
    response
}
