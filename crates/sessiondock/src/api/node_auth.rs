//! Node listener gate (`server.py` `_allowed` / `_hub_protocol` for hub
//! traffic): the TCP peer must lie inside `SESSIONDOCK_NODE_PEERS`, the request
//! must state `X-AgentHub-Protocol: 1` and present the configured credential in
//! `X-AgentHub-Node-Token` (constant-time comparison). Anything else is 403
//! before a handler runs. There is no Host gate — the hub addresses the private
//! interface — and no browser, so proxy headers never identify a peer; the
//! shared `api_policy` still refuses cross-site requests.

use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{
    config::PeerNetwork,
    error::ApiError,
    hub::{self, NodeToken},
    security,
    state::AppState,
};

/// Identity and credential of this node, loaded once at startup. `None` in
/// `AppState` means no node listener, `protocol: 0` and `node_id: null`.
pub struct NodeIdentity {
    /// 32 lowercase hex digits from the id file (minted on first start).
    pub node_id: String,
    pub token: NodeToken,
    pub peers: Vec<PeerNetwork>,
}

impl NodeIdentity {
    /// The three conditions of a hub request, peer first so an unknown
    /// address never reaches the credential comparison.
    pub fn accepts(&self, peer: Option<IpAddr>, headers: &HeaderMap) -> Result<(), ApiError> {
        let peer = peer.map(unmap).ok_or_else(peer_denied)?;
        if !self.peers.iter().any(|entry| entry.network.contains(peer)) {
            return Err(peer_denied());
        }
        let protocol = headers
            .get("x-agenthub-protocol")
            .and_then(|value| value.to_str().ok());
        if protocol != Some(&hub::PROTOCOL.to_string()) {
            return Err(auth_required());
        }
        let token = headers
            .get("x-agenthub-node-token")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if !self.token.verify(token) {
            return Err(auth_required());
        }
        Ok(())
    }
}

/// `::ffff:a.b.c.d` is the IPv4 peer it wraps (Python `_client_ip`).
fn unmap(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        v4 => v4,
    }
}

fn peer_denied() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "node_peer_denied", "forbidden")
}

fn auth_required() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "node_auth_required",
        "node authentication required",
    )
}

pub async fn node_auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(identity) = &state.node else {
        return peer_denied().into_response();
    };
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip());
    if let Err(error) = identity.accepts(peer, request.headers()) {
        return error.into_response();
    }
    security::api_policy(request, next).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn identity() -> NodeIdentity {
        NodeIdentity {
            node_id: "f".repeat(32),
            token: NodeToken::parse(&"s3cret-".repeat(8)).unwrap(),
            peers: vec![
                "10.100.100.0/24".parse().unwrap(),
                "::1/128".parse().unwrap(),
            ],
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn peer_outside_the_networks_is_refused_before_the_credential() {
        let identity = identity();
        let good = headers(&[
            ("x-agenthub-protocol", "1"),
            ("x-agenthub-node-token", &"s3cret-".repeat(8)),
        ]);
        for peer in [
            None,
            Some("10.100.101.7".parse().unwrap()),
            Some("127.0.0.1".parse().unwrap()),
        ] {
            let error = identity.accepts(peer, &good).unwrap_err();
            assert_eq!(
                (error.status, error.code),
                (StatusCode::FORBIDDEN, "node_peer_denied")
            );
            assert_eq!(error.message, "forbidden");
        }
        assert!(
            identity
                .accepts(Some("10.100.100.9".parse().unwrap()), &good)
                .is_ok()
        );
        assert!(
            identity
                .accepts(Some("::1".parse().unwrap()), &good)
                .is_ok()
        );
        // A v4-mapped v6 peer is the v4 address it wraps.
        assert!(
            identity
                .accepts(Some("::ffff:10.100.100.9".parse().unwrap()), &good)
                .is_ok()
        );
    }

    #[test]
    fn protocol_and_token_are_both_required_and_exact() {
        let identity = identity();
        let peer = Some("10.100.100.9".parse().unwrap());
        let token = "s3cret-".repeat(8);
        let cases: [&[(&str, &str)]; 6] = [
            &[],
            &[("x-agenthub-protocol", "1")],
            &[("x-agenthub-node-token", &token)],
            &[
                ("x-agenthub-protocol", "99"),
                ("x-agenthub-node-token", &token),
            ],
            &[
                ("x-agenthub-protocol", "1"),
                ("x-agenthub-node-token", &"s3cret-".repeat(7)),
            ],
            &[
                ("x-agenthub-protocol", "1"),
                ("x-agenthub-node-token", &"S3CRET-".repeat(8)),
            ],
        ];
        for (index, case) in cases.iter().enumerate() {
            let error = identity.accepts(peer, &headers(case)).unwrap_err();
            assert_eq!(
                (error.status, error.code, error.message.as_str()),
                (
                    StatusCode::FORBIDDEN,
                    "node_auth_required",
                    "node authentication required"
                ),
                "case {index}"
            );
        }
        assert!(
            identity
                .accepts(
                    peer,
                    &headers(&[
                        ("x-agenthub-protocol", "1"),
                        ("x-agenthub-node-token", &token)
                    ])
                )
                .is_ok()
        );
    }
}
