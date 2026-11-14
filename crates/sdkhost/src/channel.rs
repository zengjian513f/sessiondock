//! Opaque WebSocket text channel. No harness protocol or automatic reconnect.
use futures_util::{SinkExt, StreamExt};
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

pub async fn run(address: &std::ffi::OsStr) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        if let Some(path) = address.as_bytes().strip_prefix(b"unix://") {
            let path = std::path::Path::new(std::ffi::OsStr::from_bytes(path));
            let stream = tokio::net::UnixStream::connect(path).await?;
            let (socket, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
                .await
                .map_err(io::Error::other)?;
            return relay(socket).await;
        }
    }
    let address = address
        .to_str()
        .ok_or_else(|| io::Error::other("invalid WebSocket address"))?;
    let mut request = address.into_client_request().map_err(io::Error::other)?;
    if let Ok(token) = std::env::var("SDKHOST_CHANNEL_TOKEN") {
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}")
                .parse()
                .map_err(io::Error::other)?,
        );
    }
    let authority = request
        .uri()
        .authority()
        .ok_or_else(|| io::Error::other("missing authority"))?
        .as_str();
    let stream = tokio::net::TcpStream::connect(authority).await?;
    let (socket, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .map_err(|_| io::Error::other("WebSocket handshake failed"))?;
    relay(socket).await
}

async fn relay<S>(socket: tokio_tungstenite::WebSocketStream<S>) -> io::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    // A JSON string per record preserves newlines inside a native text frame.
    // The enclosing sdkhost owns these pipes and persists both directions.
    let (mut sink, mut stream) = socket.split();
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::stdout();
    output.write_all(b"{\"ready\":true}\n").await?;
    output.flush().await?;
    loop {
        tokio::select! {
            line = input.next_line() => {
                let Some(line) = line? else { sink.close().await.map_err(io::Error::other)?; return Ok(()); };
                let text: String = serde_json::from_str(&line)?;
                sink.send(Message::text(text)).await.map_err(io::Error::other)?;
            }
            message = stream.next() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text.to_string(),
                    Some(Ok(Message::Binary(bytes))) => String::from_utf8(bytes.to_vec()).map_err(io::Error::other)?,
                    Some(Ok(Message::Ping(bytes))) => { sink.send(Message::Pong(bytes)).await.map_err(io::Error::other)?; continue; }
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => return Err(io::Error::other(error)),
                };
                let mut bytes = serde_json::to_vec(&text)?;
                bytes.push(b'\n');
                output.write_all(&bytes).await?;
                output.flush().await?;
            }
        }
    }
}
