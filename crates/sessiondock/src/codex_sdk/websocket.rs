//! Explicit local TCP listeners. The native process and its capability token
//! are operator-managed; neither a browser nor a thread can choose this address.
use std::{net::SocketAddr, path::Path};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, handshake::client::Request};

pub const TOKEN_ENV: &str = "SESSIONDOCK_CODEX_APP_SERVER_TOKEN";

pub fn address(value: &str) -> Option<SocketAddr> {
    let authority = value.strip_prefix("ws://")?;
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    let address: SocketAddr = authority.parse().ok()?;
    (address.ip().is_loopback() && address.port() != 0).then_some(address)
}

pub fn is_websocket(path: &Path) -> bool {
    path.to_str()
        .is_some_and(|value| value.starts_with("ws://"))
}

pub fn remote_arg(path: &Path) -> String {
    if super::managed::is_managed(path) {
        super::managed::credentials(path)
            .map(|(url, _)| url)
            .unwrap_or_default()
    } else if is_websocket(path) {
        path.to_string_lossy().into_owned()
    } else {
        format!("unix://{}", path.display())
    }
}

pub(super) fn request(value: &str) -> Result<Request, &'static str> {
    address(value).ok_or("WebSocket 必须使用本机 loopback IP 和端口")?;
    let token = std::env::var(TOKEN_ENV).map_err(|_| "缺少 Codex WebSocket capability token")?;
    request_with_token(value, &token)
}

pub(super) fn request_with_token(value: &str, token: &str) -> Result<Request, &'static str> {
    address(value).ok_or("WebSocket 必须使用本机 loopback IP 和端口")?;
    if token.trim().is_empty() {
        return Err("缺少 Codex WebSocket capability token");
    }
    let mut request = value
        .into_client_request()
        .map_err(|_| "无效的 Codex WebSocket 地址")?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {token}")
            .parse()
            .map_err(|_| "无效的 Codex WebSocket capability token")?,
    );
    Ok(request)
}

pub(super) fn reachable(value: &str) -> bool {
    let Ok(token) = std::env::var(TOKEN_ENV) else {
        return false;
    };
    reachable_with_token(value, &token)
}

pub(super) fn reachable_with_token(value: &str, token: &str) -> bool {
    let Some(address) = address(value) else {
        return false;
    };
    let Ok(request) = request_with_token(value, token) else {
        return false;
    };
    let timeout = std::time::Duration::from_secs(3);
    let Ok(stream) = std::net::TcpStream::connect_timeout(&address, timeout) else {
        return false;
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
    {
        return false;
    }
    match tokio_tungstenite::tungstenite::client(request, stream) {
        Ok((mut socket, _)) => {
            let _ = socket.close(None);
            true
        }
        Err(_) => false,
    }
}
