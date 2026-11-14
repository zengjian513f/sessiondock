//! One WebSocket JSON-RPC connection over a Unix control socket or an
//! authenticated loopback TCP listener: standard HTTP Upgrade, one JSON
//! message per text frame, `"jsonrpc"` omitted. The reader routes responses to
//! their waiters and hands notifications/server requests to `Inner`.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{Endpoint, Inner};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(unix)]
const CONNECT_ATTEMPTS: u32 = 40;
#[cfg(unix)]
const CONNECT_PAUSE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
pub enum RpcError {
    /// The socket could not be reached (and could not be started).
    Unavailable(String),
    /// The connection closed or the request timed out; the call may have run.
    Disconnected,
    /// The app-server answered with a JSON-RPC error.
    Remote {
        code: i64,
        message: String,
    },
    Storage(String),
    NoActiveTurn,
    UnknownRequest,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "无法连接 Codex app-server：{reason}"),
            Self::Disconnected => f.write_str("与 Codex app-server 的连接中断；请求可能已执行"),
            Self::Remote { message, .. } => write!(f, "Codex app-server：{message}"),
            Self::Storage(message) => f.write_str(message),
            Self::NoActiveTurn => f.write_str("当前没有进行中的回合"),
            Self::UnknownRequest => f.write_str("该请求已被处理或已失效"),
        }
    }
}

type Waiter = oneshot::Sender<Result<Value, RpcError>>;

pub struct Connection {
    pub socket: std::path::PathBuf,
    channel: Arc<super::channel::Channel>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, Waiter>>,
    closed: CancellationToken,
    done: CancellationToken,
    generation: u64,
}

impl Connection {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    pub async fn open(
        inner: Arc<Inner>,
        endpoint: &Endpoint,
        generation: u64,
    ) -> Result<Arc<Self>, RpcError> {
        let mut channels = inner.channels.lock().await;
        if let Some(existing) = channels
            .get(&endpoint.socket)
            .and_then(std::sync::Weak::upgrade)
        {
            if !existing.is_closed() {
                return Ok(existing);
            }
            existing.done.cancelled().await;
        }
        let managed = super::managed::is_managed(&endpoint.socket);
        let credentials = if managed {
            let endpoint = endpoint.clone();
            Some(
                tokio::task::spawn_blocking(move || {
                    super::managed::ensure(&endpoint)?;
                    super::managed::credentials(&endpoint.socket)
                        .map_err(|e| RpcError::Unavailable(e.to_string()))
                })
                .await
                .map_err(|e| RpcError::Unavailable(e.to_string()))??,
            )
        } else {
            None
        };
        let (address, token): (std::ffi::OsString, Option<String>) =
            if let Some((url, token)) = credentials {
                (url.into(), Some(token))
            } else if super::websocket::is_websocket(&endpoint.socket) {
                let url = endpoint.socket.to_str().expect("WebSocket URL").to_owned();
                super::websocket::request(&url).map_err(|e| RpcError::Unavailable(e.into()))?;
                (
                    url.into(),
                    Some(
                        std::env::var(super::websocket::TOKEN_ENV)
                            .map_err(|e| RpcError::Unavailable(e.to_string()))?,
                    ),
                )
            } else {
                #[cfg(unix)]
                {
                    let _ = connect(endpoint).await?;
                    let mut address = std::ffi::OsString::from("unix://");
                    address.push(&endpoint.socket);
                    (address, None)
                }
                #[cfg(not(unix))]
                {
                    return Err(RpcError::Unavailable(
                        "此平台需要配置 loopback WebSocket".into(),
                    ));
                }
            };
        let channel =
            super::channel::Channel::open(&inner.host_state, endpoint, &address, token.as_deref())
                .await?;
        let connection = Arc::new(Self {
            socket: endpoint.socket.clone(),
            next_id: AtomicI64::new(channel.next_id()),
            channel,
            pending: Mutex::new(HashMap::new()),
            closed: CancellationToken::new(),
            done: CancellationToken::new(),
            generation,
        });
        connection.attach_reader();
        spawn_io(inner.clone(), connection.clone());
        if let Err(error) = connection
            .channel
            .enqueue(super::channel::Channel::initialize_frame())
        {
            connection.close();
            connection.done.cancelled().await;
            return Err(error);
        }
        let initialized = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(frame) = connection.channel.initialized() {
                    return Ok(frame);
                }
                if connection.is_closed() {
                    return Err(connection.channel.startup_error().await);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            Err(RpcError::Unavailable(
                "Codex channel initialize timed out".into(),
            ))
        });
        let initialized = match initialized {
            Ok(frame) => frame,
            Err(error) => {
                connection.close();
                connection.done.cancelled().await;
                return Err(error);
            }
        };
        if let Some(error) = initialized.get("error") {
            connection.close();
            connection.done.cancelled().await;
            // Initialization was explicitly rejected: this channel has not
            // subscribed to a thread or accepted a turn. Retire only the relay.
            let _ = connection.channel.client.signal(9).await;
            let _ = connection.channel.client.wait().await;
            return Err(RpcError::Remote {
                code: error["code"].as_i64().unwrap_or(0),
                message: error["message"]
                    .as_str()
                    .unwrap_or("initialize failed")
                    .into(),
            });
        }
        connection.send(json!({"method":"initialized","params":{}}))?;
        channels.insert(endpoint.socket.clone(), Arc::downgrade(&connection));
        Ok(connection)
    }

    pub(super) fn restore_requests(self: &Arc<Self>, inner: &Inner, thread: &str) {
        for frame in self.channel.pending(thread) {
            inner.server_request(
                self,
                frame["id"].clone(),
                frame["method"].as_str().unwrap_or(""),
                frame["params"].clone(),
            );
        }
    }

    fn attach_reader(self: &Arc<Self>) {
        CONNECTIONS
            .lock()
            .expect("connections")
            .insert(self.generation, Arc::downgrade(self));
    }

    fn send(&self, message: Value) -> Result<(), RpcError> {
        self.channel.enqueue(message)
    }

    pub async fn request(self: &Arc<Self>, method: &str, params: Value) -> Result<Value, RpcError> {
        if self.is_closed() {
            return Err(RpcError::Disconnected);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending").insert(id, tx);
        if let Err(error) = self.send(json!({"method": method, "id": id, "params": params})) {
            self.pending.lock().expect("pending").remove(&id);
            return Err(error);
        }
        // HTTP cancellation (including the Hub's shorter timeout) must not
        // cancel detection of a stalled native connection. Never replay the RPC.
        let connection = self.clone();
        let method = method.to_owned();
        tokio::spawn(async move {
            match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
                Ok(Ok(result)) => result,
                Err(_) => {
                    crate::log::warn(
                        "codex_sdk.rpc_timeout",
                        json!({"method":method,"generation":connection.generation(),
                            "timeout_seconds":REQUEST_TIMEOUT.as_secs()}),
                    );
                    // A socket can remain open while no replies are arriving.
                    // Retire it so retained subscriptions recover fresh snapshots.
                    connection.close();
                    Err(RpcError::Disconnected)
                }
                Ok(Err(_)) => Err(RpcError::Disconnected),
            }
        })
        .await
        .unwrap_or(Err(RpcError::Disconnected))
    }

    pub fn reply(&self, id: Value, result: Value) -> Result<(), RpcError> {
        if self.is_closed() {
            return Err(RpcError::Disconnected);
        }
        self.send(json!({"id": id, "result": result}))
    }

    pub fn refuse(&self, id: Value, method: &str) {
        let _ = self.send(json!({"id": id, "error": {"code": -32601,
            "message": format!("SessionDock does not handle {method}")}}));
    }

    fn resolve(&self, id: i64, outcome: Result<Value, RpcError>) {
        if let Some(waiter) = self.pending.lock().expect("pending").remove(&id) {
            let _ = waiter.send(outcome);
        }
    }

    fn close(&self) {
        self.closed.cancel();
        for (_, waiter) in self.pending.lock().expect("pending").drain() {
            let _ = waiter.send(Err(RpcError::Disconnected));
        }
        CONNECTIONS
            .lock()
            .expect("connections")
            .remove(&self.generation);
    }
}

/// The reader resolves its own connection by generation; a weak link keeps
/// a dropped connection from being held alive by its task.
static CONNECTIONS: std::sync::LazyLock<Mutex<HashMap<u64, std::sync::Weak<Connection>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn current(generation: u64) -> Option<Arc<Connection>> {
    CONNECTIONS
        .lock()
        .expect("connections")
        .get(&generation)?
        .upgrade()
}

#[cfg(unix)]
pub(super) async fn connect(endpoint: &Endpoint) -> Result<tokio::net::UnixStream, RpcError> {
    match tokio::net::UnixStream::connect(&endpoint.socket).await {
        Ok(stream) => return Ok(stream),
        Err(error) if endpoint.autostart_home.is_none() => {
            return Err(RpcError::Unavailable(format!(
                "{}：{error}",
                endpoint.socket.display()
            )));
        }
        Err(_) => {}
    }
    let home = endpoint.autostart_home.as_ref().expect("autostart home");
    let directory = endpoint.socket.parent().expect("Codex socket directory");
    crate::sdk_common::private_dir(directory).map_err(|e| RpcError::Unavailable(e.to_string()))?;
    let mut command = endpoint
        .launcher
        .as_ref()
        .map(crate::sdk_common::host::command)
        .unwrap_or_else(|| tokio::process::Command::new("codex"));
    if let Some(provider) = &endpoint.provider {
        command.args(["-c", &format!("model_provider={}", json!(provider))]);
    }
    command
        .args([
            "app-server",
            "--listen",
            &format!("unix://{}", endpoint.socket.display()),
        ])
        .env("CODEX_HOME", home);
    let record = crate::sdk_common::host::directory(
        &endpoint.host_state.join("sdk-adapters/codex-listeners"),
        endpoint.socket.as_os_str(),
    )
    .map_err(|e| RpcError::Unavailable(e.to_string()))?;
    let client = crate::sdk_common::host::instance(
        &endpoint.host_state,
        &record,
        "codex-listener",
        &command,
    )
    .await
    .map_err(|e| RpcError::Unavailable(e.to_string()))?;
    for _ in 0..CONNECT_ATTEMPTS {
        if let Ok(stream) = tokio::net::UnixStream::connect(&endpoint.socket).await {
            return Ok(stream);
        }
        if let Some(exit) = client
            .status()
            .await
            .map_err(|e| RpcError::Unavailable(e.to_string()))?
            .exit
        {
            return Err(RpcError::Unavailable(format!(
                "Codex listener exited: {exit:?}"
            )));
        }
        tokio::time::sleep(CONNECT_PAUSE).await;
    }
    Err(RpcError::Unavailable("Codex listener 启动超时".into()))
}

fn spawn_io(inner: Arc<Inner>, connection: Arc<Connection>) {
    tokio::spawn(async move {
        let channel = &connection.channel;
        let mut offset = channel.offset();
        let mut buffer = Vec::new();
        let mut sent = 0;
        let result: Result<(), RpcError> = async {
            loop {
                if inner.shutdown.is_cancelled() || connection.closed.is_cancelled() { return Ok(()); }
                let entries = channel.entries();
                for entry in &entries[sent..] { channel.write(entry).await?; sent += 1; }
                let chunk = tokio::select! {
                    _ = inner.shutdown.cancelled() => return Ok(()),
                    _ = connection.closed.cancelled() => return Ok(()),
                    _ = channel.wake.notified() => continue,
                    result = channel.client.read(sdkhost_client::Stream::Stdout, offset) => result.map_err(|e| RpcError::Unavailable(e.to_string()))?,
                };
                offset = chunk.next_offset;
                buffer.extend(&chunk.bytes);
                while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                    let line: Vec<_> = buffer.drain(..=end).collect();
                    let record: Value = serde_json::from_slice(&line).map_err(|e| RpcError::Unavailable(e.to_string()))?;
                    let consumed = offset - buffer.len() as u64;
                    if let Some(text) = record.as_str() {
                        let frame: Value = serde_json::from_str(text).map_err(|e| RpcError::Unavailable(e.to_string()))?;
                        channel.observe(&frame, consumed)?;
                        dispatch(&inner, connection.generation(), text);
                    } else { channel.observe(&Value::Null, consumed)?; }
                }
                if chunk.eof { return Err(RpcError::Disconnected); }
            }
        }.await;
        connection.close();
        inner.disconnected(
            connection.generation(),
            if result.is_ok() {
                "shutdown"
            } else {
                "hosted channel closed"
            },
        );
        connection.done.cancel();
    });
}

fn dispatch(inner: &Inner, generation: u64, text: &str) {
    let Ok(Value::Object(mut message)) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let params = message.remove("params").unwrap_or(Value::Null);
    match (method, message.remove("id")) {
        (Some(method), Some(id)) => {
            if let Some(connection) = current(generation) {
                inner.server_request(&connection, id, &method, params);
            }
        }
        (Some(method), None) => inner.notify(generation, &method, params),
        (None, Some(id)) => {
            let Some(connection) = current(generation) else {
                return;
            };
            let Some(id) = id.as_i64() else { return };
            let outcome = match message.remove("error") {
                Some(error) => Err(RpcError::Remote {
                    code: error["code"].as_i64().unwrap_or(0),
                    message: error["message"]
                        .as_str()
                        .unwrap_or("unknown error")
                        .to_string(),
                }),
                None => Ok(message.remove("result").unwrap_or(Value::Null)),
            };
            connection.resolve(id, outcome);
        }
        (None, None) => {}
    }
}
