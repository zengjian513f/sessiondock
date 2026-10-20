//! Versioned, SDK-agnostic byte transport to a persistent process owner.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}
impl Exit {
    pub fn success(&self) -> bool {
        self.code == Some(0) && self.signal.is_none()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub pid: u32,
    pub exit: Option<Exit>,
    pub stdin_closed: bool,
    pub stdout_len: u64,
    pub stderr_len: u64,
    pub stdout_eof: bool,
    pub stderr_eof: bool,
    pub output_error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub state: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadResult {
    pub bytes: Vec<u8>,
    pub next_offset: u64,
    pub eof: bool,
}
#[derive(Serialize, Deserialize)]
pub enum Request {
    Status,
    Write { id: String, bytes: Vec<u8> },
    Receipt { id: String },
    CloseStdin { id: String },
    Signal { signal: i32 },
    Read { stream: Stream, offset: u64 },
    Remove,
}
#[derive(Serialize, Deserialize)]
pub struct Response {
    pub value: Option<serde_json::Value>,
    pub error: Option<String>,
}
#[derive(Serialize, Deserialize)]
pub struct LaunchSpec {
    pub program: Vec<u8>,
    pub args: Vec<Vec<u8>>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub cwd: Option<Vec<u8>>,
}
impl LaunchSpec {
    /// Captures the current environment plus command overrides. For an env_clear
    /// command, construct LaunchSpec explicitly with the intended full environment.
    pub fn from_command(command: &tokio::process::Command) -> Self {
        let cmd = command.as_std();
        let mut env: BTreeMap<Vec<u8>, Vec<u8>> = std::env::vars_os()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect();
        for (k, v) in cmd.get_envs() {
            if let Some(v) = v {
                env.insert(k.as_bytes().to_vec(), v.as_bytes().to_vec());
            } else {
                env.remove(k.as_bytes());
            }
        }
        Self {
            program: cmd.get_program().as_bytes().to_vec(),
            args: cmd.get_args().map(|x| x.as_bytes().to_vec()).collect(),
            env: env.into_iter().collect(),
            cwd: cmd
                .get_current_dir()
                .map(|p| p.as_os_str().as_bytes().to_vec()),
        }
    }
}
pub fn os(bytes: Vec<u8>) -> std::ffi::OsString {
    std::ffi::OsString::from_vec(bytes)
}
pub fn private_dir(path: &Path) -> io::Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sdkhost directory must be private and owned by current user",
        ));
    }
    Ok(())
}
/// On Linux, resolve the socket through a directory descriptor to avoid the
/// sockaddr_un pathname length restriction on caller-owned storage paths.
pub fn socket_path(dir: &Path) -> io::Result<(PathBuf, Option<std::fs::File>)> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(dir)?;
        let path = PathBuf::from(format!(
            "/proc/self/fd/{}/control.sock",
            directory.as_raw_fd()
        ));
        Ok((path, Some(directory)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok((dir.join("control.sock"), None))
    }
}
fn location(dir: &Path, key: &str) -> io::Result<PathBuf> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sdkhost key must be a local opaque identifier",
        ));
    }
    Ok(dir.join(key))
}
#[derive(Clone, Debug)]
pub struct Client {
    dir: PathBuf,
}
impl Client {
    pub fn directory(&self) -> &Path {
        &self.dir
    }
    pub async fn attach(dir: &Path, key: &str) -> io::Result<Self> {
        let client = Self {
            dir: location(dir, key)?,
        };
        private_dir_existing(&client.dir)?;
        client.status().await?;
        Ok(client)
    }
    pub async fn launch_or_attach(
        dir: &Path,
        key: &str,
        command: &tokio::process::Command,
    ) -> io::Result<Self> {
        Self::launch_spec(dir, key, LaunchSpec::from_command(command)).await
    }
    pub async fn launch_spec(dir: &Path, key: &str, spec: LaunchSpec) -> io::Result<Self> {
        private_dir(dir)?;
        let destination = location(dir, key)?;
        let lock_path = dir.join("launch.lock");
        let _lock = tokio::task::spawn_blocking(move || -> io::Result<std::fs::File> {
            use std::os::fd::AsRawFd;
            let f = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(lock_path)?;
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(f)
        })
        .await
        .map_err(io::Error::other)??;
        if destination.exists() {
            return Self::attach(dir, key).await;
        }
        private_dir(&destination)?;
        let executable = std::env::var_os("SESSIONDOCK_SDKHOST_BIN")
            .or_else(|| std::env::var_os("SDKHOST"))
            .map(PathBuf::from)
            .unwrap_or(std::env::current_exe()?.with_file_name("sdkhost"));
        let mut cmd = tokio::process::Command::new(executable);
        cmd.arg("--dir")
            .arg(&destination)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null());
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(destination.join("host.log"))?;
        cmd.stderr(log);
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                // No host was created: only this caller's new empty instance is ours.
                let _ = std::fs::remove_dir_all(&destination);
                return Err(error);
            }
        };
        let mut input = child.stdin.take().unwrap();
        input.write_all(&serde_json::to_vec(&spec)?).await?;
        input.shutdown().await?;
        drop(input);
        let client = Self { dir: destination };
        // Reap the host only when it eventually exits, never kill on client drop.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            match client.status().await {
                Ok(_) => return Ok(client),
                Err(e) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(e);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            }
        }
    }
    async fn rpc<T: serde::de::DeserializeOwned>(&self, request: Request) -> io::Result<T> {
        let (path, _directory) = socket_path(&self.dir)?;
        let mut socket = match UnixStream::connect(path).await {
            Ok(socket) => socket,
            Err(connect_error) => return self.offline(request, connect_error).await,
        };
        if socket.peer_cred()?.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "sdkhost peer owner mismatch",
            ));
        }
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        socket.write_all(&bytes).await?;
        let mut line = Vec::new();
        BufReader::new(socket).read_until(b'\n', &mut line).await?;
        let response: Response = serde_json::from_slice(&line)?;
        if let Some(error) = response.error {
            return Err(io::Error::other(error));
        }
        serde_json::from_value(response.value.unwrap_or(serde_json::Value::Null))
            .map_err(io::Error::other)
    }
    async fn offline<T: serde::de::DeserializeOwned>(
        &self,
        request: Request,
        connect_error: io::Error,
    ) -> io::Result<T> {
        let status: Status =
            serde_json::from_slice(&tokio::fs::read(self.dir.join("status.json")).await?)?;
        let value = match request {
            Request::Status if status.exit.is_some() => serde_json::to_value(status)?,
            Request::Read { stream, offset } => {
                use tokio::io::{AsyncReadExt, AsyncSeekExt};
                let mut file = tokio::fs::File::open(self.dir.join(match stream {
                    Stream::Stdout => "stdout",
                    Stream::Stderr => "stderr",
                }))
                .await?;
                file.seek(std::io::SeekFrom::Start(offset)).await?;
                let mut bytes = vec![0; 65536];
                let n = file.read(&mut bytes).await?;
                bytes.truncate(n);
                let eof = match stream {
                    Stream::Stdout => status.stdout_eof,
                    Stream::Stderr => status.stderr_eof,
                };
                if n == 0 {
                    if let Some(error) = &status.output_error {
                        return Err(io::Error::other(error.clone()));
                    }
                    if !eof {
                        return Err(connect_error);
                    }
                }
                serde_json::to_value(ReadResult {
                    bytes,
                    next_offset: offset + n as u64,
                    eof: n == 0 && eof,
                })?
            }
            Request::Receipt { id } => {
                let log = match tokio::fs::read(self.dir.join("receipts.jsonl")).await {
                    Ok(log) => log,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
                    Err(e) => return Err(e),
                };
                let mut found = None;
                for line in log.split(|b| *b == b'\n') {
                    if let Ok(row) = serde_json::from_slice::<serde_json::Value>(line) {
                        if row.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) {
                            found = row.get("receipt").cloned();
                        }
                    }
                }
                found.unwrap_or(serde_json::Value::Null)
            }
            Request::Remove if status.exit.is_some() && status.stdout_eof && status.stderr_eof => {
                tokio::fs::remove_dir_all(&self.dir).await?;
                serde_json::Value::Null
            }
            _ => return Err(connect_error),
        };
        serde_json::from_value(value).map_err(io::Error::other)
    }
    pub async fn status(&self) -> io::Result<Status> {
        match self.rpc(Request::Status).await {
            Ok(v) => Ok(v),
            Err(e) => self.offline(Request::Status, e).await,
        }
    }
    pub async fn write(&self, id: &str, bytes: &[u8]) -> io::Result<Receipt> {
        self.rpc(Request::Write {
            id: id.into(),
            bytes: bytes.into(),
        })
        .await
    }
    pub async fn receipt(&self, id: &str) -> io::Result<Option<Receipt>> {
        match self.rpc(Request::Receipt { id: id.into() }).await {
            Ok(value) => Ok(value),
            Err(error) => {
                self.offline(Request::Receipt { id: id.into() }, error)
                    .await
            }
        }
    }
    pub async fn close_stdin(&self, id: &str) -> io::Result<Receipt> {
        self.rpc(Request::CloseStdin { id: id.into() }).await
    }
    pub async fn signal(&self, signal: i32) -> io::Result<()> {
        self.rpc(Request::Signal { signal }).await
    }
    pub async fn read(&self, stream: Stream, offset: u64) -> io::Result<ReadResult> {
        match self.rpc(Request::Read { stream, offset }).await {
            Ok(v) => Ok(v),
            Err(e) => self.offline(Request::Read { stream, offset }, e).await,
        }
    }
    pub async fn wait(&self) -> io::Result<Exit> {
        loop {
            if let Some(exit) = self.status().await?.exit {
                return Ok(exit);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    /// Only permitted after process exit; removes host records and stops the host.
    pub async fn remove(&self) -> io::Result<()> {
        match self.rpc(Request::Remove).await {
            Ok(value) => Ok(value),
            Err(error) => self.offline(Request::Remove, error).await,
        }
    }
}
fn private_dir_existing(path: &Path) -> io::Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid sdkhost directory",
        ));
    }
    Ok(())
}
