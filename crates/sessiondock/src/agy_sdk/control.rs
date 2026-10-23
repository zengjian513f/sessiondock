//! The local Connect session-control endpoints used by Agy's interactive UI.
//! Credentials and the log belong to this exact bridge runtime, never another CLI.
use crate::hub::client::{Client, Request, Target};
use serde_json::json;
use std::{
    io,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) struct Control {
    directory: PathBuf,
    pub token: String,
    pub log: PathBuf,
    temporary: bool,
}

impl Control {
    pub fn prepare(
        root: Option<&Path>,
        binding: Option<(&str, bool, u64)>,
    ) -> Result<Option<Self>, String> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| "agy_sdk_control_entropy")?;
        let fresh: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let temporary = root.is_none() || binding.is_none();
        let directory = match (root, binding) {
            (Some(root), Some((key, _, _))) => root.join("control").join(key),
            _ => {
                let mut nonce = [0u8; 16];
                getrandom::fill(&mut nonce).map_err(|_| "agy_sdk_control_entropy")?;
                let name: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
                std::env::temp_dir().join(format!("sessiondock-agy-control-{name}"))
            }
        };
        let path = directory.join("token");
        let token = match std::fs::read_to_string(&path) {
            Ok(token) => token,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // An already-running pre-upgrade CLI was not given this grant.
                // Never restart it or invent credentials to implement Stop.
                if binding.is_some_and(|(_, recovering, _)| recovering) {
                    return Ok(None);
                }
                std::fs::create_dir_all(&directory).map_err(|_| "agy_sdk_control_directory")?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                        .map_err(|_| "agy_sdk_control_directory")?;
                }
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&path).map_err(|_| "agy_sdk_control_token")?;
                file.write_all(fresh.as_bytes())
                    .map_err(|_| "agy_sdk_control_token")?;
                fresh
            }
            Err(_) => return Err("agy_sdk_control_token".into()),
        };
        Ok(Some(Self {
            log: directory.join("native.log"),
            directory,
            token,
            temporary,
        }))
    }

    pub fn cancelled(&self, turn: &str) -> bool {
        std::fs::read_to_string(self.directory.join("cancelled-turn")).is_ok_and(|id| id == turn)
    }

    pub async fn cancel(&self, pid: u32, sid: &str, turn: &str) -> Result<(), String> {
        self.request(
            pid,
            "CancelCascadeInvocation",
            json!({"cascadeId":sid,
            "killBackgroundTasks":false,"notifyParent":false}),
        )
        .await?;
        tokio::fs::write(self.directory.join("cancelled-turn"), turn)
            .await
            .map_err(|_| "agy_sdk_interrupt_receipt: 原生已接受取消，但回执未持久化。")?;
        Ok(())
    }
    pub async fn rename(&self, pid: u32, sid: &str, title: &str) -> Result<(), String> {
        self.request(
            pid,
            "UpdateConversationAnnotations",
            json!({"cascadeId":sid,
            "annotations":{"title":title},"mergeAnnotations":true}),
        )
        .await
    }

    async fn request(&self, pid: u32, method: &str, body: serde_json::Value) -> Result<(), String> {
        let log = tokio::fs::read_to_string(&self.log).await.map_err(
            |_| "agy_sdk_control_unavailable: 无法取得本实例的原生管理接口；任务继续运行。",
        )?;
        let marker = format!("Starting language server process with pid {pid}\n");
        let startup = log
            .rsplit_once(&marker)
            .map(|(_, rest)| rest)
            .ok_or("agy_sdk_control_unavailable: 原生接口进程身份不匹配；任务继续运行。")?;
        let port = startup
            .lines()
            .find_map(|line| {
                line.split_once("Language server listening on random port at ")
                    .and_then(|(_, tail)| tail.strip_suffix(" for HTTP"))
                    .and_then(|port| port.parse::<u16>().ok())
            })
            .ok_or("agy_sdk_control_unavailable: 此 CLI 未提供本机管理接口。")?;
        let target = Target {
            addr: ([127, 0, 0, 1], port).into(),
            token: String::new(),
            tls: false,
        };
        let body = serde_json::to_vec(&body).unwrap();
        let path = format!("/exa.language_server_pb.LanguageServerService/{method}");
        let headers = [
            ("Content-Type", "application/json"),
            ("Connect-Protocol-Version", "1"),
            ("x-codeium-csrf-token", self.token.as_str()),
        ];
        let request = || Request {
            method: "POST",
            target: &path,
            headers: &headers,
            body: Some(&body),
            connect: Duration::from_secs(5),
            idle: Duration::from_secs(5),
        };
        let outcome = tokio::time::timeout(Duration::from_secs(5), async {
            let client = Client::default();
            let response = client.open(&target, request()).await?;
            let mut status = response.status;
            let mut bytes = response.into_body().read_to_end(usize::MAX).await?;
            // Newer native servers use a self-signed TLS listener but retain the
            // same startup label. Retry only an explicit HTTP-to-HTTPS rejection,
            // which cannot have executed the mutation. Never retry unknown results.
            if status == 400
                && String::from_utf8_lossy(&bytes)
                    .contains("Client sent an HTTP request to an HTTPS server")
            {
                let connector = native_tls::TlsConnector::builder()
                    .danger_accept_invalid_certs(true)
                    .danger_accept_invalid_hostnames(true)
                    .build()
                    .map_err(|_| crate::hub::client::ClientError::ConnectionFailed)?;
                let socket = tokio::net::TcpStream::connect(target.addr)
                    .await
                    .map_err(|_| crate::hub::client::ClientError::ConnectionFailed)?;
                // Only our PID's loopback port and instance-owned CSRF grant.
                // The Hub's normal TLS validation is unchanged.
                let socket = tokio_native_tls::TlsConnector::from(connector)
                    .connect("localhost", socket)
                    .await
                    .map_err(|_| crate::hub::client::ClientError::ConnectionFailed)?;
                let response = client.open_on_stream(&target, request(), socket).await?;
                status = response.status;
                bytes = response.into_body().read_to_end(usize::MAX).await?;
            }
            Ok::<_, crate::hub::client::ClientError>((status, bytes))
        })
        .await
        .map_err(|_| "agy_sdk_control_timeout: 原生请求超时；未终结运行时，请核对操作结果。")?
        .map_err(
            |_| "agy_sdk_control_failed: 原生响应未完整到达；未终结运行时，请核对操作结果。",
        )?;
        if outcome.0 != 200 || serde_json::from_slice::<serde_json::Value>(&outcome.1).is_err() {
            return Err(format!(
                "agy_sdk_control_failed: 原生接口拒绝 {method}（HTTP {}）；任务继续运行。",
                outcome.0
            ));
        }
        Ok(())
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
}
