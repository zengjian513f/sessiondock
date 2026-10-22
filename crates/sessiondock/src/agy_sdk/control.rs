//! The local Connect cancellation endpoint used by Agy's interactive UI.
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
        let log = tokio::fs::read_to_string(&self.log).await.map_err(
            |_| "agy_sdk_interrupt_unavailable: 无法取得此运行实例的原生取消接口；任务继续运行。",
        )?;
        // Use only the startup block of our owned PID, not a shared/native log
        // or an arbitrary port supplied by a browser or another process.
        let marker = format!("Starting language server process with pid {pid}\n");
        let startup = log
            .rsplit_once(&marker)
            .map(|(_, rest)| rest)
            .ok_or("agy_sdk_interrupt_unavailable: 原生取消接口的进程身份不匹配；任务继续运行。")?;
        let port = startup
            .lines()
            .find_map(|line| {
                line.split_once("Language server listening on random port at ")
                    .and_then(|(_, tail)| tail.strip_suffix(" for HTTP"))
                    .and_then(|port| port.parse::<u16>().ok())
            })
            .ok_or("agy_sdk_interrupt_unavailable: 此 CLI 未提供本机取消接口；任务继续运行。")?;
        let target = Target {
            addr: ([127, 0, 0, 1], port).into(),
            token: String::new(),
            tls: false,
        };
        let body = serde_json::to_vec(
            &json!({"cascadeId":sid,"killBackgroundTasks":false,"notifyParent":false}),
        )
        .unwrap();
        let headers = [
            ("Content-Type", "application/json"),
            ("Connect-Protocol-Version", "1"),
            ("x-codeium-csrf-token", self.token.as_str()),
        ];
        let response = tokio::time::timeout(Duration::from_secs(5), async {
            let response = Client::default().open(&target, Request {
                method: "POST", target: "/exa.language_server_pb.LanguageServerService/CancelCascadeInvocation",
                headers: &headers, body: Some(&body), connect: Duration::from_secs(5), idle: Duration::from_secs(5),
            }).await.map_err(|_| "agy_sdk_interrupt_failed: 原生取消请求未获确认；未结束运行时，请核对当前回合。")?;
            let status = response.status;
            let bytes = response.into_body().read_to_end(usize::MAX).await
                .map_err(|_| "agy_sdk_interrupt_failed: 原生取消响应未完整到达；未结束运行时，请核对当前回合。")?;
            if status != 200 || serde_json::from_slice::<serde_json::Value>(&bytes).is_err() {
                return Err("agy_sdk_interrupt_failed: 原生接口拒绝取消请求；未结束运行时。".to_owned());
            }
            Ok(())
        }).await.map_err(|_| "agy_sdk_interrupt_timeout: 原生取消请求超时；未结束运行时，请核对当前回合。")?;
        response?;
        tokio::fs::write(self.directory.join("cancelled-turn"), turn)
            .await
            .map_err(|_| "agy_sdk_interrupt_receipt: 原生已接受取消，但回执未持久化。")?;
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
