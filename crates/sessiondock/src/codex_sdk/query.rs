//! Repeatable reads have no durable delivery obligation. A separate, unsubscribed
//! socket keeps polling/history out of sdkhost's input receipts and raw stream.
use super::RpcError;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::HashMap, ffi::OsString, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

type Reply = oneshot::Sender<Result<Value, RpcError>>;
type Work = (Value, Reply);
trait Transport: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Transport for T {}
type Socket = tokio_tungstenite::WebSocketStream<Box<dyn Transport>>;

pub(super) fn repeatable(method: &str) -> bool {
    matches!(
        method,
        "thread/loaded/list"
            | "thread/read"
            | "thread/turns/list"
            | "thread/list"
            | "model/list"
            | "config/read"
            | "account/rateLimits/read"
            | "thread/goal/get"
    )
}

pub(super) struct Query {
    address: OsString,
    token: Option<String>,
    sender: Mutex<Option<mpsc::UnboundedSender<Work>>>,
    closed: CancellationToken,
}
impl Query {
    pub fn new(address: OsString, token: Option<String>, closed: CancellationToken) -> Self {
        Self {
            address,
            token,
            sender: Mutex::new(None),
            closed,
        }
    }
    async fn connect(&self) -> Result<Socket, RpcError> {
        let failure = |e: String| RpcError::Unavailable(e);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            if let Some(path) = self.address.as_bytes().strip_prefix(b"unix://") {
                let path = std::path::Path::new(std::ffi::OsStr::from_bytes(path));
                let stream = tokio::net::UnixStream::connect(path)
                    .await
                    .map_err(|e| failure(e.to_string()))?;
                return tokio_tungstenite::client_async(
                    "ws://localhost/",
                    Box::new(stream) as Box<dyn Transport>,
                )
                .await
                .map(|(s, _)| s)
                .map_err(|e| failure(e.to_string()));
            }
        }
        let address = self
            .address
            .to_str()
            .ok_or_else(|| failure("invalid query address".into()))?;
        let request = super::websocket::request_with_token(
            address,
            self.token.as_deref().unwrap_or_default(),
        )
        .map_err(|e| failure(e.into()))?;
        let authority = request
            .uri()
            .authority()
            .ok_or_else(|| failure("missing authority".into()))?
            .as_str();
        let stream = tokio::net::TcpStream::connect(authority)
            .await
            .map_err(|e| failure(e.to_string()))?;
        tokio_tungstenite::client_async(request, Box::new(stream) as Box<dyn Transport>)
            .await
            .map(|(s, _)| s)
            .map_err(|e| failure(e.to_string()))
    }
    pub async fn request(self: &Arc<Self>, method: &str, params: Value) -> Result<Value, RpcError> {
        let (reply, result) = oneshot::channel();
        let mut sender = self.sender.lock().await;
        if sender.as_ref().is_none_or(|tx| tx.is_closed()) {
            let socket = tokio::time::timeout(Duration::from_secs(30), self.connect())
                .await
                .map_err(|_| RpcError::Disconnected)??;
            let (tx, rx) = mpsc::unbounded_channel();
            *sender = Some(tx);
            tokio::spawn(run(socket, rx, self.closed.clone()));
        }
        sender
            .as_ref()
            .unwrap()
            .send((json!({"method":method,"params":params}), reply))
            .map_err(|_| RpcError::Disconnected)?;
        drop(sender);
        // A stalled query must not close the owning subscription or replay input.
        tokio::time::timeout(Duration::from_secs(120), result)
            .await
            .map_err(|_| RpcError::Disconnected)?
            .map_err(|_| RpcError::Disconnected)?
    }
}

async fn run(
    mut socket: Socket,
    mut work: mpsc::UnboundedReceiver<Work>,
    closed: CancellationToken,
) {
    let mut initialize = super::channel::Channel::initialize_frame();
    initialize["id"] = json!("query:initialize");
    initialize["params"]["clientInfo"]["name"] = json!("sessiondock-query");
    if socket
        .send(Message::text(initialize.to_string()))
        .await
        .is_err()
    {
        return;
    }
    let mut ready = false;
    let mut sequence = 0u64;
    let mut pending: HashMap<String, (Reply, tokio::time::Instant)> = HashMap::new();
    let handshake_deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        // Each request owns its deadline. Continuous overlapping healthy reads
        // must not inherit the deadline of an earlier, already answered read.
        let deadline = pending
            .values()
            .map(|(_, at)| *at)
            .min()
            .unwrap_or(handshake_deadline);
        tokio::select! {
            _ = closed.cancelled() => break,
            _ = tokio::time::sleep_until(deadline), if !ready || !pending.is_empty() => break,
            next = work.recv(), if ready => {
                let Some((mut frame, reply)) = next else { break };
                sequence += 1;
                let id = format!("query:{sequence}");
                frame["id"] = json!(id);
                pending.insert(id, (reply, tokio::time::Instant::now() + Duration::from_secs(120)));
                if socket.send(Message::text(frame.to_string())).await.is_err() { break; }
            }
            next = socket.next() => {
                let text = match next {
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Binary(b))) => match String::from_utf8(b.to_vec()) { Ok(t) => t, Err(_) => break },
                    Some(Ok(Message::Ping(b))) => { if socket.send(Message::Pong(b)).await.is_err() { break; } continue; },
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => continue,
                };
                let Ok(frame) = serde_json::from_str::<Value>(&text) else { break };
                let Some(id) = frame["id"].as_str() else { continue };
                if frame.get("method").is_some() { continue; }
                if id == "query:initialize" {
                    if frame.get("error").is_some() { break; }
                    if socket.send(Message::text(json!({"method":"initialized","params":{}}).to_string())).await.is_err() { break; }
                    ready = true;
                } else if let Some((reply, _)) = pending.remove(id) {
                    let result = match frame.get("error") {
                        Some(error) => Err(RpcError::Remote { code:error["code"].as_i64().unwrap_or(0), message:error["message"].as_str().unwrap_or("query failed").into() }),
                        None => Ok(frame["result"].clone()),
                    };
                    let _ = reply.send(result);
                }
            }
        }
    }
    // Dropping waiters reports failure; only the next read opens another socket.
}
