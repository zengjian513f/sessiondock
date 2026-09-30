//! Durable cross-node clone orchestration over the authenticated node channel.
//! The Hub forwards tar chunks without buffering the archive in memory.
use super::{Client, Registry, Target, client, namespace};
use crate::transfer::TransferError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, io::Write, path::PathBuf, sync::Arc, time::Duration};

const IDLE: Duration = Duration::from_secs(300);
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Request {
    pub uid: String,
    pub target_node: String,
    pub operation_id: String,
}
#[derive(Clone, Deserialize, Serialize)]
struct Journal {
    request: Request,
    phase: String,
    result: Option<Value>,
}
pub struct Transfers {
    directory: PathBuf,
    gate: tokio::sync::Mutex<()>,
}
impl Transfers {
    pub fn open(directory: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            directory,
            gate: tokio::sync::Mutex::new(()),
        })
    }
    fn path(&self, id: &str) -> Result<PathBuf, TransferError> {
        if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err(TransferError::new("move_plan_stale", "迁移标识无效"));
        }
        Ok(self.directory.join(format!("{id}.json")))
    }
    async fn save(&self, journal: &Journal) -> Result<(), TransferError> {
        let path = self.path(&journal.request.operation_id)?;
        let raw = serde_json::to_vec(journal)?;
        tokio::task::spawn_blocking(move || {
            let temp = path.with_extension("tmp");
            let mut options = fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut out = options.open(&temp)?;
            out.write_all(&raw)?;
            out.sync_all()?;
            fs::rename(temp, &path)?;
            fs::File::open(path.parent().unwrap())?.sync_all()?;
            Ok::<_, TransferError>(())
        })
        .await
        .map_err(|e| TransferError::new("move_io", e.to_string()))?
    }
    pub async fn execute(
        &self,
        registry: Arc<Registry>,
        client: Arc<Client>,
        request: Request,
    ) -> Result<Value, TransferError> {
        let _guard = self.gate.lock().await;
        let result = self
            .run(registry.clone(), client.clone(), request.clone())
            .await;
        if result.is_err() {
            if let Ok((source, _)) = namespace::split(&request.uid, true) {
                if let Some(node) = registry.get(&source) {
                    if let Ok(target) = registry.target(&node) {
                        let source_state = call(
                            &client,
                            &target,
                            "/api/session/transfer/status",
                            &json!({"operation_id":request.operation_id}),
                            true,
                        )
                        .await;
                        let before_publication = self
                            .path(&request.operation_id)
                            .ok()
                            .and_then(|p| fs::read(p).ok())
                            .and_then(|raw| serde_json::from_slice::<Journal>(&raw).ok())
                            .is_some_and(|j| {
                                matches!(j.phase.as_str(), "planned" | "transferring")
                            });
                        if source_state
                            .as_ref()
                            .is_ok_and(|(_, value)| value["mode"] == "clone" || before_publication)
                        {
                            let _ = call(
                                &client,
                                &target,
                                "/api/session/transfer/release",
                                &json!({"operation_id":request.operation_id,"completed":false}),
                                true,
                            )
                            .await;
                        }
                    }
                }
            }
        }
        result
    }
    async fn run(
        &self,
        registry: Arc<Registry>,
        client: Arc<Client>,
        request: Request,
    ) -> Result<Value, TransferError> {
        let path = self.path(&request.operation_id)?;
        let mut journal = if path.exists() {
            let previous: Journal = serde_json::from_slice(&fs::read(&path)?)?;
            if previous.request != request {
                return Err(TransferError::new(
                    "move_conflict",
                    "操作已绑定其他迁移目标",
                ));
            }
            previous
        } else {
            Journal {
                request: request.clone(),
                phase: "planned".into(),
                result: None,
            }
        };
        if journal.phase == "complete" {
            return journal
                .result
                .ok_or_else(|| TransferError::new("move_recovery_required", "迁移结果缺失"));
        }
        let (source_id, local_uid) = namespace::split(&request.uid, true)
            .map_err(|e| TransferError::new("move_plan_stale", e.to_string()))?;
        if source_id == request.target_node {
            return Err(TransferError::new(
                "move_conflict",
                "跨机复制需要另一台机器",
            ));
        }
        let source = registry
            .get(&source_id)
            .ok_or_else(|| TransferError::new("move_node_unavailable", "源机器不可用"))?;
        let target = registry
            .get(&request.target_node)
            .ok_or_else(|| TransferError::new("move_node_unavailable", "目标机器不可用"))?;
        let source_address = registry
            .target(&source)
            .map_err(|_| TransferError::new("move_node_unavailable", "源机器不可用"))?;
        let target_address = registry
            .target(&target)
            .map_err(|_| TransferError::new("move_node_unavailable", "目标机器不可用"))?;
        self.save(&journal).await?;
        let operation = json!({"operation_id":request.operation_id});
        let source_state = call(
            &client,
            &source_address,
            "/api/session/transfer/status",
            &operation,
            true,
        )
        .await?
        .1;
        if source_state["uid"] != local_uid {
            return Err(TransferError::new("move_plan_stale", "操作与源会话不符"));
        }
        let moving = source_state["mode"] == "move";
        let mut current = call(
            &client,
            &target_address,
            "/api/session/transfer/status",
            &operation,
            false,
        )
        .await?;
        if current.0 != 404 && current.1["incoming"] != true {
            return Err(TransferError::new(
                "move_conflict",
                "目标操作并非迁移接收记录",
            ));
        }
        if current.1["phase"] != "complete" && current.1["phase"] != "ready" {
            let reserved = call(
                &client,
                &source_address,
                "/api/session/transfer/reserve",
                &operation,
                true,
            )
            .await?
            .1;
            if reserved["uid"] != local_uid {
                return Err(TransferError::new(
                    "move_plan_stale",
                    "迁移操作与所选会话不符",
                ));
            }
            if current.0 == 404 {
                let manifest = call(
                    &client,
                    &source_address,
                    "/api/session/transfer/manifest",
                    &operation,
                    true,
                )
                .await?
                .1;
                let checked = call(
                    &client,
                    &target_address,
                    "/api/session/transfer/check",
                    &manifest,
                    true,
                )
                .await?;
                if moving && checked.1["move_handoff"] != true {
                    return Err(TransferError::new(
                        "move_group_unsupported",
                        "目标版本不支持迁移归属交接",
                    ));
                }
                journal.phase = "transferring".into();
                self.save(&journal).await?;
                self.stream(&client, &source_address, &target_address, &operation)
                    .await?;
            }
            // Recheck stopped state and the source snapshot immediately before
            // target publication. Source lease fences managed launches throughout.
            call(
                &client,
                &source_address,
                "/api/session/transfer/reserve",
                &operation,
                true,
            )
            .await?;
            journal.phase = "publishing".into();
            self.save(&journal).await?;
            current = call(
                &client,
                &target_address,
                "/api/session/clone",
                &json!({"uid":local_uid,"operation_id":request.operation_id}),
                true,
            )
            .await?;
        }
        if moving && current.1["phase"] == "ready" {
            journal.phase = "switching".into();
            self.save(&journal).await?;
            call(
                &client,
                &source_address,
                "/api/session/transfer/switch",
                &operation,
                true,
            )
            .await?;
            current = call(
                &client,
                &target_address,
                "/api/session/transfer/activate",
                &operation,
                true,
            )
            .await?;
        }
        if current.1["phase"] != "complete" {
            return Err(TransferError::new(
                "move_recovery_required",
                "目标复制尚未完成",
            ));
        }
        let mut result = namespace::public_payload(current.1, &target, "/api/session/clone");
        result["uid"] = request.uid.clone().into();
        journal.phase = "releasing".into();
        journal.result = Some(result.clone());
        self.save(&journal).await?;
        if moving {
            journal.phase = "retiring".into();
            self.save(&journal).await?;
            if let Err(error) = call(
                &client,
                &source_address,
                "/api/session/transfer/retire",
                &operation,
                true,
            )
            .await
            {
                journal.phase = "cleanup_pending".into();
                self.save(&journal).await?;
                return Err(TransferError::new(
                    "move_cleanup_pending",
                    format!("目标已可继续；源端清理待重试：{}", error.message),
                ));
            }
        } else {
            call(
                &client,
                &source_address,
                "/api/session/transfer/release",
                &json!({"operation_id":request.operation_id,"completed":true}),
                true,
            )
            .await?;
        }
        journal.phase = "complete".into();
        self.save(&journal).await?;
        Ok(result)
    }
    async fn stream(
        &self,
        client: &Client,
        source: &Target,
        target: &Target,
        operation: &Value,
    ) -> Result<(), TransferError> {
        let encoded = serde_json::to_vec(operation)?;
        let response = client
            .open(
                source,
                client::Request {
                    method: "POST",
                    target: "/api/session/transfer/export",
                    headers: &[("Content-Type", "application/json")],
                    body: Some(&encoded),
                    connect: Duration::from_secs(10),
                    idle: IDLE,
                },
            )
            .await
            .map_err(network)?;
        if response.status != 200 {
            let raw = response
                .into_body()
                .read_to_end(super::JSON_LIMIT)
                .await
                .map_err(network)?;
            return Err(remote_error(&serde_json::from_slice(&raw)?));
        }
        let length = response
            .header("content-length")
            .ok_or_else(|| TransferError::new("move_format", "迁移包缺少长度"))?
            .to_owned();
        let headers = [
            ("Content-Type", "application/x-tar"),
            ("Content-Length", length.as_str()),
        ];
        let mut upload = client
            .connect(
                target,
                &client::Request {
                    method: "POST",
                    target: "/api/session/transfer/receive",
                    headers: &headers,
                    body: None,
                    connect: Duration::from_secs(10),
                    idle: IDLE,
                },
            )
            .await
            .map_err(network)?;
        let mut body = response.into_body();
        while let Some(bytes) = body.read().await.map_err(network)? {
            upload.send(&bytes).await.map_err(network)?;
        }
        let received = upload.response().await.map_err(network)?;
        let status = received.status;
        let raw = received
            .into_body()
            .read_to_end(super::JSON_LIMIT)
            .await
            .map_err(network)?;
        let result: Value = serde_json::from_slice(&raw)?;
        if status != 200 {
            return Err(remote_error(&result));
        }
        Ok(())
    }
}
fn network(error: super::ClientError) -> TransferError {
    TransferError::new("move_node_unavailable", error.message(IDLE))
}
fn remote_error(value: &Value) -> TransferError {
    TransferError::new(
        value["code"].as_str().unwrap_or("move_io"),
        value["error"].as_str().unwrap_or("节点迁移操作失败"),
    )
}
async fn call(
    client: &Client,
    target: &Target,
    path: &str,
    body: &Value,
    success: bool,
) -> Result<(u16, Value), TransferError> {
    let (status, value) = client
        .json(target, "POST", path, Some(body), IDLE)
        .await
        .map_err(network)?;
    if status != 200 && (success || status != 404) {
        return Err(remote_error(&value));
    }
    Ok((status, value))
}
