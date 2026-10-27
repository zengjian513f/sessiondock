//! Relay the native manifest without expanding it into a Hub JSON value.
use super::*;

impl Transfers {
    pub(super) async fn check_manifest(
        &self,
        client: &Client,
        source: &Target,
        target: &Target,
        operation: &Value,
        journal: &mut Journal,
        work: &Arc<crate::transfer::progress::Work>,
    ) -> Result<(u16, Value), TransferError> {
        let encoded = serde_json::to_vec(operation)?;
        let response = client
            .open(
                source,
                client::Request {
                    method: "POST",
                    target: "/api/session/transfer/manifest",
                    headers: &[("Content-Type", "application/json")],
                    body: Some(&encoded),
                    connect: IDLE,
                    idle: IDLE,
                },
            )
            .await
            .map_err(network)?;
        if response.status != 200 {
            return read_reply(response, true, None).await;
        }
        // Native projections include byte arrays: decoding them into Value can
        // multiply their memory footprint. The destination owns JSON/schema
        // validation; the Hub only relays framed bytes with backpressure.
        let length = response.header("content-length").map(str::to_owned);
        let framing = length
            .as_deref()
            .map(|n| ("Content-Length", n))
            .unwrap_or(("Transfer-Encoding", "chunked"));
        journal.phase = "checking".into();
        self.save(journal).await?;
        let mut upload = client
            .connect(
                target,
                &client::Request {
                    method: "POST",
                    target: "/api/session/transfer/check",
                    headers: &[("Content-Type", "application/json"), framing],
                    body: None,
                    connect: IDLE,
                    idle: IDLE,
                },
            )
            .await
            .map_err(network)?;
        let progress = work.task(
            "发送迁移清单",
            "bytes",
            length.as_ref().and_then(|n| n.parse().ok()),
        );
        let mut body = response.into_body();
        while let Some(bytes) = body.read().await.map_err(network)? {
            if length.is_none() {
                upload
                    .send(format!("{:x}\r\n", bytes.len()).as_bytes())
                    .await
                    .map_err(network)?;
            }
            upload.send(&bytes).await.map_err(network)?;
            if length.is_none() {
                upload.send(b"\r\n").await.map_err(network)?;
            }
            progress.add(bytes.len() as u64);
        }
        if length.is_none() {
            upload.send(b"0\r\n\r\n").await.map_err(network)?;
        }
        drop(progress);
        read_reply(upload.response().await.map_err(network)?, true, None).await
    }
}
