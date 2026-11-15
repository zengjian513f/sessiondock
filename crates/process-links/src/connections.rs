//! Durable, boot-scoped connection evidence. No commands or credentials.
use crate::{Binding, Connection, Launch, Process, Session, agent::Catalog, linux::Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectionRecord {
    pub process: Process,
    pub connection: Connection,
    pub opened_at: f64,
    pub closed_at: Option<f64>,
    pub ssh: bool,
    pub shared: bool,
    pub session: Option<Session>,
    #[serde(default)]
    pub launch_chain: Vec<Launch>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub connection: Connection,
    pub at: f64,
}

/// Arrays on disk preserve full process keys (JSON object keys cannot).
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default)]
    pub last_seen: Vec<(RecordKey, f64)>,
    pub records: Vec<ConnectionRecord>,
    pub parents: Vec<(Process, Process)>,
    pub origins: Vec<(Process, Origin)>,
    pub shared: Vec<Process>,
}

pub const DEFAULT_RETENTION_SECONDS: f64 = 24.0 * 60.0 * 60.0;
type RecordKey = (Process, Connection, u64);

pub struct Tracker {
    pub retention_seconds: f64,
    by_process: BTreeMap<Process, BTreeSet<RecordKey>>,
    by_connection: BTreeMap<Connection, BTreeSet<RecordKey>>,
    last_seen: BTreeMap<RecordKey, f64>,
    recent: BTreeMap<Process, f64>,
    records: BTreeMap<(Process, Connection, u64), ConnectionRecord>,
    senders: BTreeSet<Process>,
    parents: BTreeMap<Process, Process>,
    origins: BTreeMap<Process, Origin>,
    shared: BTreeSet<Process>,
    exited: BTreeMap<Process, f64>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::restore(Saved::default())
    }
}

impl Tracker {
    pub fn restore(saved: Saved) -> Self {
        let mut tracker = Self {
            retention_seconds: DEFAULT_RETENTION_SECONDS,
            by_process: BTreeMap::new(),
            by_connection: BTreeMap::new(),
            last_seen: saved.last_seen.into_iter().collect(),
            recent: BTreeMap::new(),
            senders: saved.records.iter().map(|r| r.process.clone()).collect(),
            records: saved
                .records
                .into_iter()
                .map(|mut record| {
                    // An outage could hide a close, a fork, or a multiplex listener.
                    // Closed spans remain usable; unfinished ones need live evidence.
                    if record.closed_at.is_none() {
                        record.shared = true;
                    }
                    (
                        (
                            record.process.clone(),
                            record.connection.clone(),
                            record.opened_at.to_bits(),
                        ),
                        record,
                    )
                })
                .collect(),
            parents: saved.parents.into_iter().collect(),
            origins: saved.origins.into_iter().collect(),
            shared: saved.shared.into_iter().collect(),
            exited: BTreeMap::new(),
        };
        tracker.reindex();
        tracker
    }

    pub fn saved(&self) -> Saved {
        Saved {
            last_seen: self
                .last_seen
                .iter()
                .map(|(p, at)| (p.clone(), *at))
                .collect(),
            records: self.records.values().cloned().collect(),
            parents: self
                .parents
                .iter()
                .map(|(c, p)| (c.clone(), p.clone()))
                .collect(),
            origins: self
                .origins
                .iter()
                .map(|(p, o)| (p.clone(), o.clone()))
                .collect(),
            shared: self.shared.iter().cloned().collect(),
        }
    }

    pub fn fork(&mut self, parent: &Process, child: &Process, at: f64) {
        self.recent.insert(child.clone(), at);
        self.parents.insert(child.clone(), parent.clone());
        if !self.senders.contains(parent) {
            return;
        }
        // A socket inherited across fork is not an exclusive SSH client socket
        // (ControlPersist may fork before its Unix listener appears).
        for key in self.by_process.get(parent).into_iter().flatten() {
            let record = self.records.get_mut(key).unwrap();
            if record.closed_at.is_none() {
                record.shared = true;
            }
        }
    }

    pub fn received(&mut self, process: Process, connection: Connection, at: f64) {
        // Re-exec with the same inherited tuple must not move its launch time.
        if self
            .origin(&process)
            .is_some_and(|o| o.connection == connection)
        {
            return;
        }
        self.recent.insert(process.clone(), at);
        self.origins.insert(process, Origin { connection, at });
    }

    pub fn opened(&mut self, process: Process, connection: Connection, at: f64, ssh: bool) {
        // Other protocols only matter as evidence that an old SSH tuple was
        // reused; do not retain an unrelated machine-wide TCP history.
        if !ssh && !self.by_connection.contains_key(&connection) {
            return;
        }
        // A shell can fork a helper, then exec SSH in the same incarnation.
        // Those earlier children never inherited this not-yet-open socket.
        // fork() marks connections already open at the fork; explicit shared
        // transport observations remain sticky across connection creation.
        let shared = self.shared.contains(&process);
        self.senders.insert(process.clone());
        self.recent.insert(process.clone(), at);
        let key = (process.clone(), connection.clone(), at.to_bits());
        self.last_seen.insert(key.clone(), at);
        self.by_process
            .entry(process.clone())
            .or_default()
            .insert(key.clone());
        self.by_connection
            .entry(connection.clone())
            .or_default()
            .insert(key);
        self.records
            .entry((process.clone(), connection.clone(), at.to_bits()))
            .or_insert(ConnectionRecord {
                closed_at: self.exited.get(&process).copied(),
                process,
                connection,
                opened_at: at,
                ssh,
                shared,
                session: None,
                launch_chain: Vec::new(),
            });
    }

    pub fn closed(&mut self, process: &Process, connection: &Connection, at: f64) {
        if let Some(key) = self
            .by_process
            .get(process)
            .into_iter()
            .flatten()
            .rev()
            .find(|key| {
                let record = &self.records[*key];
                record.connection == *connection
                    && record.opened_at <= at
                    && record.closed_at.is_none()
            })
            .cloned()
        {
            self.records.get_mut(&key).unwrap().closed_at = Some(at);
        }
    }

    pub fn exit(&mut self, process: &Process, at: f64) {
        self.exited.insert(process.clone(), at);
        if !self.senders.contains(process) {
            return;
        }
        for key in self.by_process.get(process).into_iter().flatten() {
            let record = self.records.get_mut(key).unwrap();
            record.closed_at.get_or_insert(at);
        }
    }

    pub fn shared(&mut self, process: &Process) {
        self.shared.insert(process.clone());
        for key in self.by_process.get(process).into_iter().flatten() {
            let record = self.records.get_mut(key).unwrap();
            record.shared = true;
        }
    }

    pub fn gap(&mut self) {
        // Event loss might hide a listener/fork. Keep evidence as ambiguous;
        // never manufacture an exclusive association from a damaged stream.
        for record in self.records.values_mut() {
            record.shared = true;
        }
    }

    pub fn origin(&self, process: &Process) -> Option<&Origin> {
        let mut current = process;
        let mut seen = BTreeSet::new();
        while seen.insert(current) {
            if let Some(origin) = self.origins.get(current) {
                return Some(origin);
            }
            current = self.parents.get(current)?;
        }
        None
    }

    /// Fork events can arrive before their parent's binding, or while a
    /// snapshot/catalog refresh still predates that fork. Resolve the retained
    /// incarnation graph again instead of depending on immediate inheritance.
    pub fn ancestor_binding<'a>(
        &self,
        process: &Process,
        snapshot: &Snapshot,
        bindings: &'a BTreeMap<Process, Binding>,
    ) -> Option<&'a Binding> {
        let mut current = self.parents.get(process)?;
        let mut seen = BTreeSet::new();
        while seen.insert(current) {
            if self.shared.contains(current)
                || snapshot
                    .entries
                    .get(&current.pid)
                    .is_some_and(|e| e.process == *current && e.shared_parent)
            {
                return None;
            }
            if let Some(binding) = bindings.get(current) {
                return Some(binding);
            }
            current = self.parents.get(current)?;
        }
        None
    }

    pub fn update(
        &mut self,
        snapshot: &Snapshot,
        bindings: &BTreeMap<Process, Binding>,
        catalog: &Catalog,
        at: f64,
    ) {
        self.prune(snapshot, at);
        for entry in snapshot.entries.values() {
            if !entry.shared_parent
                && let Some(parent) = snapshot.entries.get(&entry.parent)
                && !parent.shared_parent
                && parent.process.start <= entry.process.start
            {
                self.parents
                    .entry(entry.process.clone())
                    .or_insert_with(|| parent.process.clone());
            }
            if entry.multiplexed {
                self.shared(&entry.process);
            }
        }
        // Snapshot fallback handles pre-existing processes without replacing
        // the successful-exec timestamp captured before a short parent exits.
        for entry in snapshot.entries.values() {
            if let Some(connection) = &entry.connection {
                let mut first = entry;
                let mut seen = BTreeSet::new();
                while seen.insert(first.process.clone()) {
                    let Some(parent) = snapshot.entries.get(&first.parent) else {
                        break;
                    };
                    if parent.shared_parent || parent.connection.as_ref() != Some(connection) {
                        break;
                    }
                    first = parent;
                }
                self.received(first.process.clone(), connection.clone(), first.started_at);
            }
        }
        self.attribute(bindings, catalog);
    }

    pub fn attribute(&mut self, bindings: &BTreeMap<Process, Binding>, catalog: &Catalog) {
        let owners: BTreeMap<_, _> = catalog
            .owners
            .iter()
            .map(|o| (&o.process, &o.session))
            .collect();
        for record in self.records.values_mut() {
            attribute_record(record, &self.parents, bindings, &owners);
        }
    }

    pub fn attribute_exiting(
        &mut self,
        process: &Process,
        bindings: &BTreeMap<Process, Binding>,
        catalog: &Catalog,
    ) {
        if let Some(keys) = self.by_process.get(process) {
            let owners = catalog
                .owners
                .iter()
                .map(|o| (&o.process, &o.session))
                .collect();
            for key in keys {
                attribute_record(
                    self.records.get_mut(key).unwrap(),
                    &self.parents,
                    bindings,
                    &owners,
                );
            }
        }
    }

    fn reindex(&mut self) {
        self.by_process.clear();
        self.by_connection.clear();
        for key in self.records.keys() {
            self.by_process
                .entry(key.0.clone())
                .or_default()
                .insert(key.clone());
            self.by_connection
                .entry(key.1.clone())
                .or_default()
                .insert(key.clone());
        }
        self.senders = self.by_process.keys().cloned().collect();
    }

    fn prune(&mut self, snapshot: &Snapshot, at: f64) {
        let live = |r: &ConnectionRecord| {
            snapshot.entries.get(&r.process.pid).is_some_and(|entry| {
                entry.process == r.process
                    && (!entry.socket_scan_complete || entry.sockets.contains(&r.connection))
            })
        };
        let cutoff = at - self.retention_seconds - 2.0;
        for (key, record) in &self.records {
            if live(record) {
                self.last_seen.insert(key.clone(), at);
            }
        }
        let expired: BTreeSet<_> = self
            .records
            .iter()
            .filter(|(key, r)| {
                !live(r)
                    && r.closed_at.unwrap_or_else(|| {
                        self.last_seen
                            .get(*key)
                            .copied()
                            .unwrap_or(r.opened_at)
                            .max(r.opened_at)
                    }) < cutoff
            })
            .map(|(key, _)| key.clone())
            .collect();
        // Keep the first subsequent connect bounding each retained unfinished
        // span. Dropping that witness must not revive a superseded tuple.
        let mut witnesses = BTreeSet::new();
        for (key, record) in &self.records {
            if record.closed_at.is_none()
                && !expired.contains(key)
                && let Some(next) = self.by_connection[&record.connection]
                    .iter()
                    .filter(|k| self.records[*k].opened_at > record.opened_at)
                    .min_by(|a, b| {
                        self.records[*a]
                            .opened_at
                            .total_cmp(&self.records[*b].opened_at)
                    })
            {
                witnesses.insert(next.clone());
            }
        }
        if !expired.is_empty() {
            self.records
                .retain(|key, _| !expired.contains(key) || witnesses.contains(key));
            self.reindex();
        }
        self.last_seen
            .retain(|key, _| self.records.contains_key(key));
        self.recent.retain(|_, seen| *seen + 2.0 >= at);
        // Only retained evidence, live descendants and not-yet-sampled events
        // retain ancestry. Removed evidence cannot pin dead graphs forever.
        let mut needed: BTreeSet<Process> = snapshot
            .entries
            .values()
            .map(|e| e.process.clone())
            .chain(self.records.values().map(|r| r.process.clone()))
            .chain(self.recent.keys().cloned())
            .collect();
        let mut pending: Vec<_> = needed.iter().cloned().collect();
        while let Some(process) = pending.pop() {
            if let Some(parent) = self.parents.get(&process)
                && needed.insert(parent.clone())
            {
                pending.push(parent.clone());
            }
        }
        self.parents.retain(|p, _| needed.contains(p));
        self.origins.retain(|p, _| needed.contains(p));
        self.shared.retain(|p| needed.contains(p));
        self.exited.retain(|p, _| needed.contains(p));
    }

    pub fn records(&self) -> Vec<ConnectionRecord> {
        self.records.values().cloned().collect()
    }
}

fn enrich(record: &mut ConnectionRecord, binding: &Binding) {
    if !record.session.as_ref().is_some_and(|session| {
        session.node_id == binding.session.node_id
            && session.source == binding.session.source
            && session.sid == binding.session.sid
    }) {
        return;
    }
    for launch in &binding.launch_chain {
        if !record
            .launch_chain
            .iter()
            .any(|old| old.process == launch.process)
        {
            record.launch_chain.push(launch.clone());
        }
    }
}

fn attribute_record(
    record: &mut ConnectionRecord,
    parents: &BTreeMap<Process, Process>,
    bindings: &BTreeMap<Process, Binding>,
    owners: &BTreeMap<&Process, &Session>,
) {
    let mut current = &record.process;
    let mut seen = BTreeSet::new();
    while seen.insert(current) {
        if let Some(owner) = owners.get(current) {
            record.session.get_or_insert_with(|| (*owner).clone());
            if let Some(binding) = bindings.get(current) {
                enrich(record, binding);
            }
            break;
        }
        if let Some(binding) = bindings.get(current) {
            record
                .session
                .get_or_insert_with(|| binding.session.clone());
            enrich(record, binding);
            break;
        }
        let Some(parent) = parents.get(current) else {
            break;
        };
        current = parent;
    }
}
