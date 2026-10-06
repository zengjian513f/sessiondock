//! Shared process identities, SSH lineage and resource aggregation.
//! No application transport, credentials or workload management.
//! Observations describe exact process incarnations and direct SSH sockets.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod agent;
pub mod connections;
pub mod engine;
pub mod linux;
pub mod resource_summary;

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
    /// Only on a session's owning CLI process: the other session that
    /// launched it (nearest bound parent, the process it was forked from, or
    /// its SSH initiator). Kept for the incarnation once observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawner: Option<Session>,
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
    /// First successful SSH exec in this inherited connection lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_at: Option<f64>,
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
    /// Event evidence is separate so old coordinators cannot mistake an exited
    /// client's retained connection for a currently established socket.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<connections::ConnectionRecord>,
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
    struct Candidate<'a> {
        source: &'a Report,
        process: &'a Process,
        session: Option<&'a Session>,
        chain: &'a [Launch],
        start: f64,
        end: Option<f64>,
        eligible: bool,
    }
    let mut outgoing: BTreeMap<&Connection, Vec<Candidate<'_>>> = BTreeMap::new();
    for report in reports.iter().filter(|r| r.version == 1 && r.supported) {
        for edge in &report.outgoing {
            if report
                .connections
                .iter()
                .any(|r| r.process == edge.process && r.connection == edge.connection)
            {
                continue;
            }
            outgoing
                .entry(&edge.connection)
                .or_default()
                .push(Candidate {
                    source: report,
                    process: &edge.process,
                    session: Some(&edge.session),
                    chain: &edge.launch_chain,
                    start: edge.started_at,
                    end: None,
                    eligible: true,
                });
        }
        for record in &report.connections {
            let live_exclusive = report
                .outgoing
                .iter()
                .any(|edge| edge.process == record.process && edge.connection == record.connection);
            outgoing
                .entry(&record.connection)
                .or_default()
                .push(Candidate {
                    source: report,
                    process: &record.process,
                    session: record.session.as_ref(),
                    chain: &record.launch_chain,
                    start: record.opened_at,
                    end: record.closed_at,
                    eligible: record.ssh
                        && !record.shared
                        && (record.closed_at.is_some() || live_exclusive),
                });
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
                    let at = incoming.connection_at.unwrap_or(incoming.started_at);
                    if !at.is_finite() {
                        return None;
                    }
                    let mut candidates =
                        outgoing.get(&incoming.connection)?.iter().filter(|edge| {
                            edge.start.is_finite()
                                && at + 2.0 >= edge.start
                                && edge
                                    .end
                                    .is_none_or(|end| end.is_finite() && at <= end + 2.0)
                        });
                    let edge = candidates.next()?;
                    // Ambiguous/unknown clients also participate in this check.
                    // A reused tuple cannot revive a closed launch, and clock-skew
                    // overlap is left unassigned rather than choosing a session.
                    if candidates.next().is_some() || !edge.eligible {
                        return None;
                    }
                    let source = edge.source;
                    let session = edge.session?;
                    let launcher = ProcessKey {
                        node_id: source.node_id.clone(),
                        boot_id: source.boot_id.clone(),
                        process: edge.process.clone(),
                    };
                    let mut launch_chain = vec![Launch {
                        process: launcher.clone(),
                        session: session.clone(),
                    }];
                    for launch in edge.chain {
                        if !launch_chain.iter().any(|v| v.process == launch.process) {
                            launch_chain.push(launch.clone());
                        }
                    }
                    Some(Link {
                        launch_chain,
                        process: incoming.process.clone(),
                        session: session.clone(),
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
