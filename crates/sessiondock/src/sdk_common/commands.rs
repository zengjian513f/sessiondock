//! Persistent native command channel. The worker and in-flight calls outlive HTTP/service clients.
use super::{host, save_atomic};
use sdkhost_client::{Client, Stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub(crate) struct Commands {
    root: PathBuf,
    directory: PathBuf,
    profile: crate::lifecycle::launcher::CliProfile,
    client: tokio::sync::Mutex<Option<Client>>,
}
#[derive(Serialize, Deserialize)]
struct Call {
    id: String,
    offset: u64,
    host: String,
    frame: Value,
}
impl Commands {
    pub fn new(
        root: PathBuf,
        name: &str,
        scope: &Path,
        profile: crate::lifecycle::launcher::CliProfile,
    ) -> io::Result<Self> {
        let directory = host::directory(
            &root.join("sdk-adapters/commands").join(name),
            scope.as_os_str(),
        )?;
        Ok(Self {
            root,
            directory,
            profile,
            client: tokio::sync::Mutex::new(None),
        })
    }
    async fn client(&self) -> io::Result<Client> {
        let mut slot = self.client.lock().await;
        if let Some(client) = &*slot
            && client.status().await?.exit.is_none()
        {
            return Ok(client.clone());
        }
        let executable = std::fs::canonicalize(&self.profile.executable)?;
        let command = host::command(&self.profile);
        // The host worker inherits the same sanitized environment as the native CLI.
        let mut spec = sdkhost_client::LaunchSpec::from_command(&command);
        let mut worker = tokio::process::Command::new(sdkhost_client::executable()?);
        worker
            .arg("--commands")
            .arg(self.directory.join("capture"))
            .arg(executable)
            .args(&self.profile.args);
        let worker_spec = sdkhost_client::LaunchSpec::from_command(&worker);
        spec.program = worker_spec.program;
        spec.args = worker_spec.args;
        let client =
            host::instance_spec(&self.root, &self.directory, "command-channel", spec).await?;
        *slot = Some(client.clone());
        Ok(client)
    }
    pub async fn run(
        &self,
        cwd: &Path,
        args: &[String],
        identity: Option<&str>,
        shutdown: &CancellationToken,
    ) -> io::Result<Value> {
        if shutdown.is_cancelled() {
            return Err(io::Error::other("SDK channel shutdown"));
        }
        let client = self.client().await?;
        let receipt = identity
            .map(|id| {
                host::directory(&self.directory.join("calls"), std::ffi::OsStr::new(id))
                    .map(|dir| dir.join("call.json"))
            })
            .transpose()?;
        let call = match receipt.as_ref().map(std::fs::read) {
            Some(Ok(bytes)) => serde_json::from_slice::<Call>(&bytes)?,
            Some(Err(e)) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {
                let id = super::uuid_v4().map_err(io::Error::other)?;
                let call = Call {
                    frame: json!({"id":id,"args":args,"cwd":sdkhost_client::platform::encode(cwd.as_os_str())}),
                    id,
                    offset: client.status().await?.stdout_len,
                    host: client
                        .directory()
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                };
                if let Some(path) = &receipt {
                    save_atomic(path, &call)?;
                }
                call
            }
        };
        // A prior invocation is always read from its original host, even after worker exit.
        let client = if client.directory().file_name().unwrap() == call.host.as_str() {
            client
        } else {
            Client::attach(&self.root.join("sdkhost"), &call.host).await?
        };
        let mut bytes = serde_json::to_vec(&call.frame)?;
        bytes.push(b'\n');
        let written = match client.receipt(&call.id).await? {
            Some(r) => r,
            None => client.write(&call.id, &bytes).await?,
        };
        if written.state != "written" {
            return Err(io::Error::other(
                "native command delivery unknown; will not resend",
            ));
        }
        let mut offset = call.offset;
        let mut buffer = Vec::new();
        let mut first_record = true;
        loop {
            let chunk = tokio::select! {
                _ = shutdown.cancelled() => return Err(io::Error::other("SDK channel detached; native call continues")),
                chunk = client.read(Stream::Stdout, offset) => chunk?,
            };
            offset = chunk.next_offset;
            buffer.extend(chunk.bytes);
            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = buffer.drain(..=end).collect();
                // The captured byte offset may fall inside another concurrent
                // response. Our own response starts after this call is written.
                let response = match serde_json::from_slice::<Value>(&line) {
                    Ok(value) => value,
                    Err(_) if first_record => {
                        first_record = false;
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                first_record = false;
                if response["id"] == call.id {
                    if let Some(error) = response["error"].as_str() {
                        return Err(io::Error::other(error.to_owned()));
                    }
                    return Ok(response["result"].clone());
                }
            }
            if chunk.eof {
                return Err(io::Error::other(
                    "native command channel exited; outcome unknown",
                ));
            }
        }
    }
}
