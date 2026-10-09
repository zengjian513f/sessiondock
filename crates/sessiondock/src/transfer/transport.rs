//! Configured data paths. Session identity, publication and ownership stay with
//! the transfer transaction; transports only deliver an immutable private tar.
use super::TransferError;
#[cfg(unix)]
use super::{coordination, progress};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub default: DefaultTransport,
    #[serde(default)]
    pub networks: Vec<Network>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultTransport {
    #[default]
    Hub,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    pub name: String,
    pub transport: DirectTransport,
    #[serde(default)]
    pub priority: i64,
    pub nodes: BTreeMap<String, Endpoint>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectTransport {
    Scp,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub address: String,
    pub user: String,
    pub port: u16,
}

impl Endpoint {
    pub fn validate(&self) -> Result<(), TransferError> {
        // These are administrator configuration fields used in an SSH operand,
        // never shell code or a command-line option.
        if self.address.is_empty()
            || self.address.starts_with('-')
            || !self
                .address
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".-:".contains(&c))
            || self.user.is_empty()
            || self.user.starts_with('-')
            || !self
                .user
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            || self.port == 0
        {
            return Err(TransferError::new(
                "move_format",
                "SCP 连接地址、用户或端口无效",
            ));
        }
        Ok(())
    }
}

impl Policy {
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let policy: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        for network in &policy.networks {
            if network.name.is_empty()
                || network
                    .nodes
                    .keys()
                    .any(|id| !crate::hub::identity::is_node_id(id))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "transfer network requires a name and persistent node IDs",
                ));
            }
            for endpoint in network.nodes.values() {
                endpoint
                    .validate()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            }
        }
        Ok(policy)
    }

    pub fn route(&self, source: &str, target: &str) -> Option<&Network> {
        if source == target {
            return None;
        }
        // Earlier configuration entries win equal-priority ties.
        self.networks
            .iter()
            .enumerate()
            .filter(|(_, n)| n.nodes.contains_key(source) && n.nodes.contains_key(target))
            .max_by_key(|(index, n)| (n.priority, std::cmp::Reverse(*index)))
            .map(|(_, n)| n)
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Archive {
    pub operation_id: String,
    pub path: std::path::PathBuf,
    pub bytes: u64,
}

impl Archive {
    pub fn validate(&self) -> Result<(), TransferError> {
        let name = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if self.operation_id.len() != 36
            || !self
                .operation_id
                .bytes()
                .all(|c| c.is_ascii_hexdigit() || c == b'-')
            || !self.path.is_absolute()
            || self
                .path
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                != Some(&self.operation_id)
            || !name.starts_with("export-")
            || !name.ends_with(".tar")
            || self
                .path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(TransferError::new("move_format", "SCP 迁移包与操作不符"));
        }
        Ok(())
    }
}

fn unavailable(message: &str) -> TransferError {
    TransferError::new("move_transport_unavailable", message)
}

/// Runs on the receiving node as its service user, using that user's existing
/// SSH identity and known_hosts. No Hub/node API credential enters SSH argv.
#[cfg(unix)]
pub fn pull(
    endpoint: &Endpoint,
    archive: &Archive,
    destination: &Path,
    shutdown: &tokio_util::sync::CancellationToken,
) -> Result<(), TransferError> {
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    endpoint.validate()?;
    archive.validate()?;
    coordination::check()?;
    let address = if endpoint.address.contains(':') {
        format!("[{}]", endpoint.address)
    } else {
        endpoint.address.clone()
    };
    // SFTP's source operand still uses glob expansion. Escape only glob
    // metacharacters so literal spaces, brackets and backslashes remain valid.
    let mut remote_path = String::new();
    for c in archive.path.to_string_lossy().chars() {
        if "\\*?[]".contains(c) {
            remote_path.push('\\');
        }
        remote_path.push(c);
    }
    let remote = format!("{}@{}:{}", endpoint.user, address, remote_path);
    // Force SFTP: remote paths are protocol data, never remote shell input.
    let mut command = Command::new("scp");
    command
        .args(["-s", "-B", "-q", "-P", &endpoint.port.to_string()])
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "ServerAliveInterval=10",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ControlPath=none",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ClearAllForwardings=yes",
        ])
        .arg("--")
        .arg(remote)
        .arg(destination)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    #[cfg(target_os = "linux")]
    {
        let parent = std::process::id();
        // scp handles SIGTERM by stopping its ssh child as well.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent as i32 {
                    libc::raise(libc::SIGTERM);
                }
                Ok(())
            });
        }
    }
    let child = command
        .spawn()
        .map_err(|_| unavailable("SCP 无法启动，改用 Hub 中转"))?;
    struct Process(Option<std::process::Child>);
    impl Drop for Process {
        fn drop(&mut self) {
            if let Some(child) = &mut self.0 {
                // The private group includes scp's ssh child. Reap before the
                // caller removes partial data or starts another transport.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
            }
        }
    }
    let mut process = Process(Some(child));
    let counter = progress::Task::new("SCP 接收会话包", "bytes", Some(archive.bytes));
    let mut last_bytes = 0;
    let mut advanced = Instant::now();
    loop {
        coordination::check()?;
        if shutdown.is_cancelled() {
            return Err(TransferError::new(
                "move_cancelled",
                "节点正在停止，迁移已中断",
            ));
        }
        let bytes = std::fs::metadata(destination).map(|m| m.len()).unwrap_or(0);
        counter.set(bytes);
        if bytes != last_bytes {
            last_bytes = bytes;
            advanced = Instant::now();
        }
        if let Some(status) = process.0.as_mut().unwrap().try_wait()? {
            process.0 = None;
            if !status.success() {
                return Err(unavailable("SCP 连接或传输失败，改用 Hub 中转"));
            }
            if bytes != archive.bytes {
                return Err(TransferError::new("move_format", "SCP 迁移包长度不符"));
            }
            return Ok(());
        }
        if advanced.elapsed() >= Duration::from_secs(60) {
            return Err(unavailable("SCP 传输无进展，改用 Hub 中转"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(not(unix))]
pub fn pull(
    _: &Endpoint,
    _: &Archive,
    _: &Path,
    _: &tokio_util::sync::CancellationToken,
) -> Result<(), TransferError> {
    Err(unavailable("此平台不支持 SCP 迁移，改用 Hub 中转"))
}
