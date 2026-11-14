//! Windows provider listeners, independent of the web-service lifetime.
//! A private per-provider record and capability token bind reconnects to the
//! same native process; observation never starts a replacement.
use super::{Endpoint, RpcError, websocket};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::time::{Duration, Instant};
use std::{fs, io, path::Path};

#[derive(Serialize, Deserialize)]
struct Record {
    url: String,
    pid: u32,
    started: u64,
    provider: String,
}

pub fn is_managed(path: &Path) -> bool {
    cfg!(windows)
        && path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(".listener.json"))
}

fn read(path: &Path) -> io::Result<Record> {
    serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
}

pub fn credentials(path: &Path) -> io::Result<(String, String)> {
    let record = read(path)?;
    if websocket::address(&record.url).is_none() {
        return Err(io::Error::other("invalid managed Codex loopback endpoint"));
    }
    let token = fs::read_to_string(path.with_extension("token"))?;
    Ok((record.url, token))
}

// These Windows identity queries do not yield: they read the named PID using
// a query-only handle and compare its token owner with the current user.
#[cfg(windows)]
fn identity(pid: u32) -> Result<u64, crate::runtime::process::IdentityFailure> {
    use futures_util::FutureExt;
    crate::runtime::process::observe(pid, true)
        .now_or_never()
        .expect("Windows identity query is synchronous")
        .map(|p| p.start_time.0)
}

#[cfg(windows)]
pub(super) fn ensure(endpoint: &Endpoint) -> Result<(), RpcError> {
    let failure = |e: io::Error| RpcError::Unavailable(e.to_string());
    let path = &endpoint.socket;
    let directory = path
        .parent()
        .ok_or_else(|| failure(io::Error::other("listener directory missing")))?;
    if endpoint.autostart_home.is_some() {
        crate::sdk_common::private_dir(directory).map_err(failure)?;
    }
    // The OS lock releases even if the launching service crashes. Concurrent
    // browser/TUI/service requests all inspect the record under the same lock.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(endpoint.autostart_home.is_some())
        .truncate(false)
        .open(path.with_extension("lock"))
        .map_err(failure)?;
    lock.lock().map_err(failure)?;
    let record = match read(path) {
        Ok(record) => Some(record),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(failure(error)),
    };
    if let Some(record) = record {
        if record.pid == 0 {
            return Err(RpcError::Unavailable(
                "Codex listener 启动记录未完成；请检查私有 listener 日志".into(),
            ));
        }
        match identity(record.pid) {
            Ok(started) if started == record.started => {
                if record.provider != endpoint.provider.as_deref().unwrap_or("openai") {
                    return Err(RpcError::Unavailable(
                        "Codex listener provider identity mismatch".into(),
                    ));
                }
                let (url, token) = credentials(path).map_err(failure)?;
                if websocket::reachable_with_token(&url, &token) {
                    return Ok(());
                }
                // A living process may be starting or temporarily unavailable.
                // Never replace it or start a second writer for the same provider.
                return Err(RpcError::Unavailable(
                    "Codex listener 存活但暂不可连接".into(),
                ));
            }
            Err(crate::runtime::process::IdentityFailure::NotVisible) | Ok(_) => {}
            Err(error) => {
                return Err(RpcError::Unavailable(format!(
                    "Codex listener identity: {error:?}"
                )));
            }
        }
    }
    let home = endpoint
        .autostart_home
        .as_ref()
        .ok_or_else(|| RpcError::Unavailable("Codex listener 未运行".into()))?;
    let profile = endpoint
        .launcher
        .as_ref()
        .ok_or_else(|| RpcError::Unavailable("未配置 Codex launcher".into()))?;
    let provider = endpoint.provider.as_deref().unwrap_or("openai");
    let reservation =
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(failure)?;
    let url = format!("ws://{}", reservation.local_addr().map_err(failure)?);
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| RpcError::Unavailable(e.to_string()))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let token_path = path.with_extension("token");
    fs::write(&token_path, &token).map_err(failure)?;
    let mut command = crate::sdk_common::host::command(profile);
    command
        .env("CODEX_HOME", home)
        .current_dir(home)
        .args([
            "-c",
            &format!("model_provider={}", serde_json::json!(provider)),
            "app-server",
            "--listen",
            &url,
            "--ws-auth",
            "capability-token",
            "--ws-token-file",
        ])
        .arg(&token_path);
    // Persist launch intent before CreateProcess. If this service dies between
    // spawn and recording the child identity, a later service must not create
    // an untracked duplicate. An incomplete intent remains visibly unavailable.
    crate::sdk_common::save_atomic(
        path,
        &Record {
            url: url.clone(),
            pid: 0,
            started: 0,
            provider: provider.to_owned(),
        },
    )
    .map_err(failure)?;
    drop(reservation);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    let result = runtime.block_on(async {
        let binding = crate::sdk_common::host::directory(
            &endpoint.host_state.join("sdk-adapters/codex-listeners"),
            path.as_os_str(),
        )
        .map_err(failure)?;
        let client = crate::sdk_common::host::instance(
            &endpoint.host_state,
            &binding,
            "codex-listener",
            &command,
        )
        .await
        .map_err(failure)?;
        let status = client.status().await.map_err(failure)?;
        let started = identity(status.pid)
            .map_err(|e| RpcError::Unavailable(format!("Codex listener identity: {e:?}")))?;
        crate::sdk_common::save_atomic(
            path,
            &Record {
                url: url.clone(),
                pid: status.pid,
                started,
                provider: provider.into(),
            },
        )
        .map_err(failure)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if websocket::reachable_with_token(&url, &token) {
                return Ok(());
            }
            if client.status().await.map_err(failure)?.exit.is_some() {
                return Err(RpcError::Unavailable("Codex listener 退出".into()));
            }
            if Instant::now() >= deadline {
                return Err(RpcError::Unavailable("Codex listener 启动超时".into()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    result
}

#[cfg(not(windows))]
pub(super) fn ensure(_endpoint: &Endpoint) -> Result<(), RpcError> {
    Err(RpcError::Unavailable(
        "受控 TCP listener 仅用于 Windows".into(),
    ))
}
