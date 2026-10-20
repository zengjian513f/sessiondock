//! Non-Unix builds retain the API and explicitly report unavailable transport.
use std::{
    io,
    path::{Path, PathBuf},
};
#[derive(Clone, Copy, Debug)]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Clone, Debug)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}
impl Exit {
    pub fn success(&self) -> bool {
        self.code == Some(0) && self.signal.is_none()
    }
}
#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
pub struct Receipt {
    pub state: String,
}
#[derive(Clone, Debug)]
pub struct ReadResult {
    pub bytes: Vec<u8>,
    pub next_offset: u64,
    pub eof: bool,
}
#[derive(Clone, Debug)]
pub struct Client {
    dir: PathBuf,
}
fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "sdkhost transport requires Unix",
    ))
}
impl Client {
    pub fn directory(&self) -> &Path {
        &self.dir
    }
    pub async fn attach(_: &Path, _: &str) -> io::Result<Self> {
        unsupported()
    }
    pub async fn launch_or_attach(
        _: &Path,
        _: &str,
        _: &tokio::process::Command,
    ) -> io::Result<Self> {
        unsupported()
    }
    pub async fn status(&self) -> io::Result<Status> {
        unsupported()
    }
    pub async fn write(&self, _: &str, _: &[u8]) -> io::Result<Receipt> {
        unsupported()
    }
    pub async fn receipt(&self, _: &str) -> io::Result<Option<Receipt>> {
        unsupported()
    }
    pub async fn close_stdin(&self, _: &str) -> io::Result<Receipt> {
        unsupported()
    }
    pub async fn signal(&self, _: i32) -> io::Result<()> {
        unsupported()
    }
    pub async fn read(&self, _: Stream, _: u64) -> io::Result<ReadResult> {
        unsupported()
    }
    pub async fn wait(&self) -> io::Result<Exit> {
        unsupported()
    }
    pub async fn remove(&self) -> io::Result<()> {
        unsupported()
    }
}
