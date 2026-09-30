//! Shared process-link protocol. No SessionDock, HTTP, credentials or CPU logic.
//! Observations describe exact process incarnations and direct SSH sockets.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod agent;
pub mod engine;
pub mod linux;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub start: u64,
}

/// Global identity for a process incarnation, also used by resource samples.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcessKey {
    pub node_id: String,
    pub boot_id: String,
    pub process: Process,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Connection {
    pub client_ip: String,
    pub client_port: u16,
    pub server_ip: String,
    pub server_port: u16,
}

impl Connection {
    pub fn parse(value: &str) -> Option<Self> {
        let parts: Vec<_> = value.split_whitespace().collect();
        if parts.len() != 4 {
            return None;
        }
        fn ip(value: &str) -> Option<String> {
            let value: std::net::IpAddr = value.parse().ok()?;
            Some(match value {
                std::net::IpAddr::V6(v) => v
                    .to_ipv4_mapped()
                    .map_or_else(|| v.to_string(), |v| v.to_string()),
                v => v.to_string(),
            })
        }
        Some(Self {
            client_ip: ip(parts[0])?,
            client_port: parts[1].parse().ok()?,
            server_ip: ip(parts[2])?,
            server_port: parts[3].parse().ok()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub node_id: String,
    pub source: String,
    pub sid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<f64>,
}

/// One observed SSH launch; nearest launch first in a causal chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Launch {
    pub process: ProcessKey,
    pub session: Session,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub process: Process,
    /// The local CLI owner, or the remote initiator for a plain remote command.
    pub session: Session,
    /// SSH launch causality is independent of native session creation/nesting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator: Option<Session>,
    pub first_observed_at: f64,
    #[serde(default)]
    pub launch_chain: Vec<Launch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outgoing {
    #[serde(default)]
    pub launch_chain: Vec<Launch>,
    pub process: Process,
    pub started_at: f64,
    pub connection: Connection,
    pub session: Session,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Incoming {
    pub process: Process,
    pub started_at: f64,
    pub connection: Connection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub version: u32,
    pub node_id: String,
    pub boot_id: String,
    pub supported: bool,
    pub sampled_at: f64,
    pub outgoing: Vec<Outgoing>,
    pub incoming: Vec<Incoming>,
    pub bindings: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collector: Option<agent::CollectorStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    #[serde(default)]
    pub launch_chain: Vec<Launch>,
    pub process: Process,
    pub launcher: ProcessKey,
    pub observed_at: f64,
    pub session: Session,
    pub connection: Connection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Published {
    pub boot_id: String,
    pub links: Vec<Link>,
}

/// Exactly one launching process must explain a connection. Never choose by
/// directory, timestamps, machine suffix or the nearest-looking session.
pub fn correlate(reports: &[Report]) -> BTreeMap<String, Published> {
    let mut outgoing: BTreeMap<&Connection, Vec<(&Report, &Outgoing)>> = BTreeMap::new();
    for report in reports.iter().filter(|r| r.version == 1 && r.supported) {
        for edge in &report.outgoing {
            outgoing
                .entry(&edge.connection)
                .or_default()
                .push((report, edge));
        }
    }
    reports
        .iter()
        .filter(|r| r.version == 1 && r.supported)
        .map(|report| {
            let links = report
                .incoming
                .iter()
                .filter_map(|incoming| {
                    let candidates = outgoing.get(&incoming.connection)?;
                    if candidates.len() != 1 {
                        return None;
                    }
                    let (source, edge) = candidates[0];
                    // An old detached command may still carry a tuple that has been
                    // reused by a newer connection. Allow two seconds of clock skew.
                    if incoming.started_at + 2.0 < edge.started_at {
                        return None;
                    }
                    let launcher = ProcessKey {
                        node_id: source.node_id.clone(),
                        boot_id: source.boot_id.clone(),
                        process: edge.process.clone(),
                    };
                    let mut launch_chain = vec![Launch {
                        process: launcher.clone(),
                        session: edge.session.clone(),
                    }];
                    for launch in &edge.launch_chain {
                        if !launch_chain.iter().any(|v| v.process == launch.process) {
                            launch_chain.push(launch.clone());
                        }
                    }
                    Some(Link {
                        launch_chain,
                        process: incoming.process.clone(),
                        session: edge.session.clone(),
                        launcher,
                        observed_at: report.sampled_at,
                        connection: incoming.connection.clone(),
                    })
                })
                .collect();
            (
                report.node_id.clone(),
                Published {
                    boot_id: report.boot_id.clone(),
                    links,
                },
            )
        })
        .collect()
}
