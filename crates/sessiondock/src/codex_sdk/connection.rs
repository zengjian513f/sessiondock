//! One WebSocket JSON-RPC connection over the app-server's Unix control
//! socket (`--listen unix://`: standard HTTP Upgrade, one JSON message per
//! text frame, `"jsonrpc"` omitted). The reader task routes responses to
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
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{Endpoint, Inner};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_ATTEMPTS: u32 = 40;
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
    out: mpsc::UnboundedSender<String>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, Waiter>>,
    closed: CancellationToken,
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
        #[cfg(unix)]
        {
            let stream = connect(endpoint).await?;
            let (socket, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
                .await
                .map_err(|error| RpcError::Unavailable(format!("握手失败：{error}")))?;
            let closed = CancellationToken::new();
            let connection = Arc::new(Self {
                socket: endpoint.socket.clone(),
                out: spawn_io(inner, socket, generation, closed.clone())?,
                next_id: AtomicI64::new(1),
                pending: Mutex::new(HashMap::new()),
                closed,
                generation,
            });
            connection.attach_reader();
            connection
                .request(
                    "initialize",
                    json!({
                        "clientInfo": {"name": "sessiondock", "title": "SessionDock",
                            "version": env!("CARGO_PKG_VERSION")},
                        "capabilities": {"experimentalApi": true},
                    }),
                )
                .await?;
            connection.send(json!({"method": "initialized", "params": {}}));
            Ok(connection)
        }
        #[cfg(not(unix))]
        {
            let _ = (inner, endpoint, generation);
            Err(RpcError::Unavailable("此平台不支持".into()))
        }
    }

    fn attach_reader(self: &Arc<Self>) {
        CONNECTIONS
            .lock()
            .expect("connections")
            .insert(self.generation, Arc::downgrade(self));
    }

    fn send(&self, message: Value) {
        let _ = self.out.send(message.to_string());
    }

    pub async fn request(self: &Arc<Self>, method: &str, params: Value) -> Result<Value, RpcError> {
        if self.is_closed() {
            return Err(RpcError::Disconnected);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending").insert(id, tx);
        self.send(json!({"method": method, "id": id, "params": params}));
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
        self.send(json!({"id": id, "result": result}));
        Ok(())
    }

    pub fn refuse(&self, id: Value, method: &str) {
        self.send(json!({"id": id, "error": {"code": -32601,
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
async fn connect(endpoint: &Endpoint) -> Result<tokio::net::UnixStream, RpcError> {
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
    if let Some(provider) = &endpoint.provider {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let directory = endpoint.socket.parent().expect("provider socket directory");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|e| RpcError::Unavailable(e.to_string()))?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(endpoint.socket.with_extension("log"))
            .map_err(|e| RpcError::Unavailable(e.to_string()))?;
        let mut command = tokio::process::Command::new("codex");
        command
            .args([
                "-c",
                &format!("model_provider={}", json!(provider)),
                "app-server",
                "--listen",
                &format!("unix://{}", endpoint.socket.display()),
            ])
            .env("CODEX_HOME", home)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(log)
            .kill_on_drop(false);
        // Match detached native hosts: service restart must not kill a turn.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|e| RpcError::Unavailable(e.to_string()))?;
        for _ in 0..CONNECT_ATTEMPTS {
            if let Ok(stream) = tokio::net::UnixStream::connect(&endpoint.socket).await {
                return Ok(stream);
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|e| RpcError::Unavailable(e.to_string()))?
            {
                return Err(RpcError::Unavailable(format!(
                    "provider {provider} 进程退出：{status}；日志：{}",
                    endpoint.socket.with_extension("log").display()
                )));
            }
            tokio::time::sleep(CONNECT_PAUSE).await;
        }
        return Err(RpcError::Unavailable(format!(
            "provider {provider} 启动超时"
        )));
    }
    // The same daemon a default `codex` TUI starts; it outlives SessionDock.
    let started = tokio::process::Command::new("codex")
        .args(["app-server", "daemon", "start"])
        .env("CODEX_HOME", home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false)
        .status();
    match tokio::time::timeout(Duration::from_secs(60), started).await {
        Ok(Ok(status)) if status.success() => {}
        Ok(Ok(status)) => {
            return Err(RpcError::Unavailable(format!(
                "codex app-server daemon start 退出：{status}"
            )));
        }
        Ok(Err(error)) => {
            return Err(RpcError::Unavailable(format!(
                "无法启动 codex app-server daemon：{error}"
            )));
        }
        Err(_) => {
            return Err(RpcError::Unavailable(
                "启动 codex app-server daemon 超时".into(),
            ));
        }
    }
    let mut last = String::new();
    for _ in 0..CONNECT_ATTEMPTS {
        match tokio::net::UnixStream::connect(&endpoint.socket).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error.to_string(),
        }
        tokio::time::sleep(CONNECT_PAUSE).await;
    }
    Err(RpcError::Unavailable(format!(
        "{}：{last}",
        endpoint.socket.display()
    )))
}

#[cfg(unix)]
fn spawn_io(
    inner: Arc<Inner>,
    socket: tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    generation: u64,
    closed: CancellationToken,
) -> Result<mpsc::UnboundedSender<String>, RpcError> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (mut sink, mut stream) = socket.split();
    let (out, mut outgoing) = mpsc::unbounded_channel::<String>();
    let shutdown = inner.shutdown.clone();
    tokio::spawn(async move {
        let reason = loop {
            tokio::select! {
                _ = shutdown.cancelled() => break "shutdown",
                _ = closed.cancelled() => break "request timed out",
                message = outgoing.recv() => {
                    let Some(message) = message else { break "closed" };
                    if sink.send(Message::text(message)).await.is_err() {
                        break "write failed";
                    }
                }
                frame = stream.next() => {
                    let text = match frame {
                        Some(Ok(Message::Text(text))) => text.to_string(),
                        Some(Ok(Message::Binary(bytes))) => String::from_utf8_lossy(&bytes).into_owned(),
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = sink.send(Message::Pong(payload)).await;
                            continue;
                        }
                        Some(Ok(Message::Close(_))) | None => break "closed",
                        Some(Ok(_)) => continue,
                        Some(Err(_)) => break "read failed",
                    };
                    dispatch(&inner, generation, &text);
                }
            }
        };
        if let Some(connection) = current(generation) {
            connection.close();
        }
        inner.disconnected(generation, reason);
    });
    Ok(out)
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
