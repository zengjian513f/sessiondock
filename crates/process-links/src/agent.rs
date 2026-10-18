//! Local protocol shared by resource-agent and application adapters.
use crate::{Binding, Process, Published, Report, Session};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_SOCKET: &str = "/run/resource-agent/agent.sock";
pub fn socket_path() -> PathBuf {
    std::env::var_os("RESOURCE_AGENT_SOCKET").map_or_else(|| DEFAULT_SOCKET.into(), PathBuf::from)
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CollectorStatus {
    pub service: String,
    pub events: String,
    pub lost_events: u64,
    #[serde(default)]
    pub ssh_connections: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Owner {
    pub process: Process,
    pub session: Session,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub node_id: String,
    pub boot_id: String,
    pub owners: Vec<Owner>,
    pub sessions: Vec<Session>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", content = "data", rename_all = "snake_case")]
pub enum Request {
    Health,
    Report,
    Resources,
    Usage,
    Probe {
        enabled: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease_seconds: Option<u64>,
    },
    Catalog(Catalog),
    Publish(Published),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sample {
    pub process: Process,
    pub cpu_seconds: f64,
    /// Resident pages, not proportional memory. Never label as PSS.
    pub rss_bytes: u64,
    pub threads: u64,
    pub read_bytes: Option<u64>,
    pub write_bytes: Option<u64>,
    #[serde(default)]
    pub metrics: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMeasurement {
    pub session: Session,
    pub metrics: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resources {
    pub version: u32,
    pub node_id: String,
    pub boot_id: String,
    pub sampled_at: f64,
    pub availability: String,
    pub method: String,
    pub samples: Vec<Sample>,
    pub unavailable: Vec<String>,
    #[serde(default)]
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub metric_availability: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub sessions: Vec<serde_json::Value>,
    /// Measurements native to a session group, never duplicated per process.
    #[serde(default)]
    pub session_measurements: Vec<SessionMeasurement>,
}
/// One bounded-time local exchange. Application requests never enter SSH's path.
#[cfg(unix)]
fn response<T: serde::de::DeserializeOwned>(
    path: &Path,
    request: &Request,
) -> std::io::Result<Option<T>> {
    use std::io::{BufRead, BufReader, BufWriter, Write};
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(path)?;
    let timeout = Some(std::time::Duration::from_secs(2));
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    // serde emits punctuation and short fields separately. Buffer the catalog
    // instead of making thousands of one-byte socket writes per observation.
    let mut writer = BufWriter::new(&mut stream);
    serde_json::to_writer(&mut writer, request)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    drop(writer);
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    #[derive(Deserialize)]
    struct Envelope<T> {
        ok: bool,
        result: Option<T>,
        error: Option<String>,
    }
    let response: Envelope<T> = serde_json::from_str(&line)?;
    if !response.ok {
        return Err(std::io::Error::other(
            response.error.as_deref().unwrap_or("agent unavailable"),
        ));
    }
    Ok(response.result)
}
#[cfg(unix)]
pub fn request(path: &Path, request: &Request) -> std::io::Result<serde_json::Value> {
    response(path, request).map(|result| result.unwrap_or(serde_json::Value::Null))
}

#[cfg(not(unix))]
pub fn request(_: &Path, _: &Request) -> std::io::Result<serde_json::Value> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "resource-agent requires Unix",
    ))
}
/// Decode on the blocking caller directly into the consumer's type. Avoid
/// constructing, cloning and then consuming a boot-long JSON value tree.
#[cfg(unix)]
pub fn request_typed<T: serde::de::DeserializeOwned>(
    path: &Path,
    request: &Request,
) -> std::io::Result<T> {
    response(path, request)?.ok_or_else(|| std::io::Error::other("agent response has no result"))
}
#[cfg(not(unix))]
pub fn request_typed<T: serde::de::DeserializeOwned>(_: &Path, _: &Request) -> std::io::Result<T> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "resource-agent requires Unix",
    ))
}

/// Local liveness/catalog consumers need bindings, not historical TCP evidence.
/// Skipped fields are still parsed/validated, but never allocated as Values or
/// connection records. The full report remains available to the coordinator.
pub fn bindings(path: &Path, request: &Request) -> std::io::Result<Report> {
    #[derive(Deserialize)]
    struct Bindings {
        version: u32,
        node_id: String,
        boot_id: String,
        supported: bool,
        sampled_at: f64,
        bindings: Vec<Binding>,
    }
    let result: Bindings = request_typed(path, request)?;
    Ok(Report {
        version: result.version,
        node_id: result.node_id,
        boot_id: result.boot_id,
        supported: result.supported,
        sampled_at: result.sampled_at,
        bindings: result.bindings,
        outgoing: Vec::new(),
        incoming: Vec::new(),
        connections: Vec::new(),
        collector: None,
    })
}
