//! Common process ownership. Adapters build commands; only sdkhost starts them.
use sdkhost_client::{Client, Stream};
use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub(crate) fn state_dir(configured: Option<&Path>) -> PathBuf {
    static TEMP: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
        std::env::temp_dir().join(format!(
            "sessiondock-sdk-{}",
            super::uuid_v4().expect("OS randomness")
        ))
    });
    configured
        .map(Path::to_path_buf)
        .unwrap_or_else(|| TEMP.clone())
}

pub(crate) fn command(profile: &crate::lifecycle::launcher::CliProfile) -> Command {
    let mut command = Command::new(&profile.executable);
    command.args(&profile.args);
    for name in &profile.env_remove {
        command.env_remove(name);
    }
    command.envs(&profile.env);
    for name in crate::lifecycle::launcher::DENIED_ENV {
        command.env_remove(name);
    }
    command
}

pub(crate) async fn read_all(client: &Client, stream: Stream) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = client.read(stream, offset).await?;
        offset = chunk.next_offset;
        bytes.extend(chunk.bytes);
        if chunk.eof {
            return Ok(bytes);
        }
    }
}

pub(crate) fn exit_status(exit: sdkhost_client::Exit) -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(
            exit.code
                .map_or_else(|| exit.signal.unwrap_or(1), |code| code << 8),
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(exit.code.unwrap_or(1) as u32)
    }
}

/// A disposable query, never a user turn. Timeout stops only its owned host.
pub(crate) async fn query(
    root: &Path,
    prefix: &str,
    command: &Command,
    timeout: Duration,
    shutdown: CancellationToken,
) -> io::Result<std::process::Output> {
    super::private_dir(root)?;
    let key = format!("{prefix}-{}", super::uuid_v4().map_err(io::Error::other)?);
    let mut spec = sdkhost_client::LaunchSpec::from_command(command);
    spec.lifetime_ms = Some(timeout.as_millis() as u64);
    let client = Client::launch_spec(&root.join("sdkhost"), &key, spec).await?;
    // A fast query may exit before the client attaches. EOF is already satisfied
    // in that case; its durable output is still the successful query result.
    if let Err(error) = client.close_stdin("query-eof").await {
        let status = client.status().await?;
        if !status.stdin_closed && status.exit.is_none() {
            return Err(error);
        }
    }
    let result = tokio::select! {
        _ = shutdown.cancelled() => Err(io::Error::other("SDK query shutdown")),
        result = tokio::time::timeout(timeout, async {
            let (stdout, stderr, exit) = tokio::try_join!(read_all(&client, Stream::Stdout), read_all(&client, Stream::Stderr), client.wait())?;
            Ok(std::process::Output { status: exit_status(exit), stdout, stderr })
        }) => result.unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "SDK query timed out"))),
    };
    if result.is_err() {
        let _ = client.signal(9).await;
        let _ = client.wait().await;
    }
    // remove requires both streams at EOF; failed cleanup leaves inspectable records.
    let _ = client.remove().await;
    result
}

/// Lossless runtime identity scoped beneath private SessionDock state.
pub(crate) fn directory(root: &Path, identity: &std::ffi::OsStr) -> io::Result<PathBuf> {
    let encoded: String = identity
        .as_encoded_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut path = root.to_path_buf();
    for part in encoded.as_bytes().chunks(96) {
        path.push(std::str::from_utf8(part).unwrap());
    }
    super::private_dir(&path)?;
    Ok(path)
}

/// Bind an explicitly requested runtime once; an unknown host is never replaced.
pub(crate) async fn instance(
    root: &Path,
    directory: &Path,
    prefix: &str,
    command: &Command,
) -> io::Result<Client> {
    instance_spec(
        root,
        directory,
        prefix,
        sdkhost_client::LaunchSpec::from_command(command),
    )
    .await
}

pub(crate) async fn instance_spec(
    root: &Path,
    directory: &Path,
    prefix: &str,
    spec: sdkhost_client::LaunchSpec,
) -> io::Result<Client> {
    super::private_dir(directory)?;
    let lock_path = directory.join("launch.lock");
    let _lock = tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        file.lock()?;
        Ok::<_, io::Error>(file)
    })
    .await
    .map_err(io::Error::other)??;
    let path = directory.join("host.json");
    match std::fs::read(&path) {
        Ok(bytes) => {
            let key: String = serde_json::from_slice(&bytes)?;
            let client = Client::attach(&root.join("sdkhost"), &key).await?;
            if client.status().await?.exit.is_none() {
                return Ok(client);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let key = format!("{prefix}-{}", super::uuid_v4().map_err(io::Error::other)?);
    super::save_atomic(&path, &key)?;
    Client::launch_spec(&root.join("sdkhost"), &key, spec).await
}

/// Source-neutral line assembly over durable stdout. No record-size limit.
pub(crate) struct Lines {
    pub client: Client,
    offset: u64,
    buffer: Vec<u8>,
    eof: bool,
}
impl Lines {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            offset: 0,
            buffer: Vec::new(),
            eof: false,
        }
    }
    pub async fn next(&mut self) -> io::Result<Option<String>> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = self.buffer.drain(..=end).collect();
                return String::from_utf8(line).map(Some).map_err(io::Error::other);
            }
            if self.eof {
                return if self.buffer.is_empty() {
                    Ok(None)
                } else {
                    String::from_utf8(std::mem::take(&mut self.buffer))
                        .map(Some)
                        .map_err(io::Error::other)
                };
            }
            let chunk = self.client.read(Stream::Stdout, self.offset).await?;
            self.offset = chunk.next_offset;
            self.eof = chunk.eof;
            self.buffer.extend(chunk.bytes);
        }
    }
}
