//! Attribution state independent of a session UI or transport.
use crate::{
    Binding, Incoming, Link, Outgoing, Process, Published, Report, Session,
    agent::{Catalog, CollectorStatus},
    linux::Snapshot,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Saved {
    pub node_id: String,
    pub boot_id: String,
    pub catalog: Catalog,
    pub links: Vec<Link>,
    pub bindings: Vec<Binding>,
}
pub struct Engine {
    pub node_id: String,
    pub boot_id: String,
    pub catalog: Catalog,
    pub bindings: BTreeMap<Process, Binding>,
    pub links: BTreeMap<Process, Link>,
    incoming: BTreeMap<Process, crate::Connection>,
}
impl Engine {
    pub fn new(node_id: String, boot_id: String, saved: Option<Saved>) -> Self {
        let saved = saved
            .filter(|s| s.node_id == node_id && s.boot_id == boot_id)
            .unwrap_or_default();
        Self {
            node_id,
            boot_id,
            catalog: saved.catalog,
            bindings: saved
                .bindings
                .into_iter()
                .map(|b| (b.process.clone(), b))
                .collect(),
            links: saved
                .links
                .into_iter()
                .map(|l| (l.process.clone(), l))
                .collect(),
            incoming: BTreeMap::new(),
        }
    }
    pub fn catalog(&mut self, catalog: Catalog) -> bool {
        if catalog.node_id != self.node_id || catalog.boot_id != self.boot_id {
            return false;
        }
        self.catalog = catalog;
        true
    }
    pub fn saved(&self) -> Saved {
        Saved {
            node_id: self.node_id.clone(),
            boot_id: self.boot_id.clone(),
            catalog: self.catalog.clone(),
            links: self.links.values().cloned().collect(),
            bindings: self.bindings.values().cloned().collect(),
        }
    }
    pub fn fork(&mut self, parent: &Process, child: Process, at: f64) {
        if let Some(mut binding) = self.bindings.get(parent).cloned() {
            binding.process = child.clone();
            binding.first_observed_at = at;
            self.bindings.insert(child, binding);
        }
    }
    pub fn exit(&mut self, process: &Process) {
        self.bindings.remove(process);
        self.links.remove(process);
    }
    pub fn publish(&mut self, published: Published, snapshot: &Snapshot) -> usize {
        if published.boot_id != self.boot_id {
            return 0;
        }
        let mut count = 0;
        for link in published.links {
            if self.incoming.get(&link.process) != Some(&link.connection)
                || !snapshot
                    .entries
                    .get(&link.process.pid)
                    .is_some_and(|p| p.process == link.process)
                || link.session.sid.is_empty()
                || link.session.source.is_empty()
            {
                continue;
            }
            if let Some(saved) = self.links.get_mut(&link.process) {
                if saved.launcher == link.launcher {
                    for launch in link.launch_chain {
                        if !saved
                            .launch_chain
                            .iter()
                            .any(|v| v.process == launch.process)
                        {
                            saved.launch_chain.push(launch);
                        }
                    }
                }
            } else {
                self.links.insert(link.process.clone(), link);
            }
            count += 1;
        }
        count
    }
    pub fn update(&mut self, snapshot: &Snapshot, at: f64, status: CollectorStatus) -> Report {
        if snapshot.boot_id != self.boot_id {
            self.boot_id = snapshot.boot_id.clone();
            self.bindings.clear();
            self.links.clear();
            self.catalog = Catalog::default();
        }
        let live = |p: &Process| {
            snapshot
                .entries
                .get(&p.pid)
                .is_some_and(|e| e.process == *p)
        };
        self.bindings.retain(|p, _| live(p));
        self.links.retain(|p, _| live(p));
        let owners: BTreeMap<_, _> = self
            .catalog
            .owners
            .iter()
            .filter(|o| live(&o.process))
            .map(|o| (o.process.clone(), o.session.clone()))
            .collect();
        let sessions: HashMap<_, _> = self
            .catalog
            .sessions
            .iter()
            .map(|s| ((s.source.clone(), s.sid.to_lowercase()), s))
            .collect();
        let mut outgoing = Vec::new();
        let mut incoming = Vec::new();
        let mut bindings = BTreeMap::new();
        self.incoming.clear();
        for entry in snapshot.entries.values() {
            let mut current = entry.process.pid;
            let mut seen = HashSet::new();
            let mut local: Option<Session> = None;
            let mut inherited: Option<Binding> = None;
            let mut remote: Option<Link> = self.links.get(&entry.process).cloned();
            let mut connection = None;
            while current > 1 && seen.insert(current) {
                let Some(ancestor) = snapshot.entries.get(&current) else {
                    break;
                };
                if ancestor.shared_parent {
                    break;
                }
                if local.is_none() {
                    local = owners.get(&ancestor.process).cloned().or_else(|| {
                        ancestor
                            .identities
                            .iter()
                            .find_map(|key| sessions.get(key).map(|s| (*s).clone()))
                    });
                    if inherited.is_none() {
                        inherited = self.bindings.get(&ancestor.process).cloned();
                    }
                }
                if remote.is_none() {
                    remote = self.links.get(&ancestor.process).cloned();
                }
                if connection.is_none() {
                    connection = ancestor.connection.clone();
                }
                current = ancestor.parent;
            }
            if let Some(connection) = connection {
                self.incoming
                    .insert(entry.process.clone(), connection.clone());
                incoming.push(Incoming {
                    process: entry.process.clone(),
                    started_at: entry.started_at,
                    connection,
                });
            }
            let session = local
                .or_else(|| inherited.as_ref().map(|b| b.session.clone()))
                .or_else(|| remote.as_ref().map(|l| l.session.clone()));
            if let Some(session) = session {
                let initiator = remote
                    .as_ref()
                    .map(|l| l.session.clone())
                    .or_else(|| inherited.as_ref().and_then(|b| b.initiator.clone()));
                let launch_chain = remote
                    .as_ref()
                    .map(|l| l.launch_chain.clone())
                    .or_else(|| inherited.as_ref().map(|b| b.launch_chain.clone()))
                    .unwrap_or_default();
                let first_observed_at = self
                    .bindings
                    .get(&entry.process)
                    .map(|b| b.first_observed_at)
                    .or_else(|| remote.as_ref().map(|l| l.observed_at))
                    .unwrap_or(at);
                let binding = Binding {
                    process: entry.process.clone(),
                    session: session.clone(),
                    initiator,
                    launch_chain: launch_chain.clone(),
                    first_observed_at,
                };
                bindings.insert(entry.process.clone(), binding);
                if !entry.multiplexed && entry.sockets.len() == 1 {
                    outgoing.push(Outgoing {
                        process: entry.process.clone(),
                        started_at: entry.started_at,
                        session,
                        launch_chain,
                        connection: entry.sockets[0].clone(),
                    });
                }
            }
        }
        self.bindings = bindings;
        Report {
            version: 1,
            node_id: self.node_id.clone(),
            boot_id: self.boot_id.clone(),
            supported: !self.boot_id.is_empty(),
            sampled_at: at,
            bindings: self.bindings.values().cloned().collect(),
            outgoing,
            incoming,
            collector: Some(status),
        }
    }
}
