//! Durable, boot-scoped connection evidence. No commands or credentials.
use crate::{Binding, Connection, Launch, Process, Session, agent::Catalog, linux::Snapshot};
use im::OrdMap;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Unbounded},
    sync::Arc,
};

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
pub type RecordKey = (Process, Connection, u64);

impl ConnectionRecord {
    pub fn key(&self) -> RecordKey {
        (
            self.process.clone(),
            self.connection.clone(),
            self.opened_at.to_bits(),
        )
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRecord {
    #[serde(flatten)]
    pub record: ConnectionRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<f64>,
}
impl StoredRecord {
    fn retention_start(&self) -> Time {
        Time(self.record.closed_at.unwrap_or_else(|| {
            self.last_seen
                .unwrap_or(self.record.opened_at)
                .max(self.record.opened_at)
        }))
    }
}

pub type Records = OrdMap<RecordKey, Arc<StoredRecord>>;

/// Snapshots retain the same tree nodes for untouched evidence.
#[derive(Clone, Default)]
pub struct Evidence {
    pub records: Records,
    pub parents: OrdMap<Process, Process>,
    pub origins: OrdMap<Process, Origin>,
    pub shared: OrdMap<Process, ()>,
}
impl From<Saved> for Evidence {
    fn from(saved: Saved) -> Self {
        let last_seen: BTreeMap<_, _> = saved.last_seen.into_iter().collect();
        Self {
            records: saved
                .records
                .into_iter()
                .map(|record| {
                    let key = record.key();
                    let last_seen = record
                        .closed_at
                        .is_none()
                        .then(|| last_seen.get(&key).copied())
                        .flatten();
                    (key, Arc::new(StoredRecord { record, last_seen }))
                })
                .collect(),
            parents: saved.parents.into_iter().collect(),
            origins: saved.origins.into_iter().collect(),
            shared: saved.shared.into_iter().map(|p| (p, ())).collect(),
        }
    }
}

#[derive(Clone, Copy)]
struct Time(f64);
impl PartialEq for Time {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Time {}
impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Time {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.0 == other.0 {
            Ordering::Equal
        } else {
            self.0.total_cmp(&other.0)
        }
    }
}

#[derive(Default)]
struct DueWhileLive {
    // An incomplete scan keeps all records of this incarnation live.
    sockets: Option<Vec<Connection>>,
    keys: BTreeSet<RecordKey>,
}

#[derive(PartialEq)]
struct Attribution {
    session: Session,
    binding: Option<(Session, Vec<Launch>)>,
}

pub struct Tracker {
    pub retention_seconds: f64,
    evidence: Evidence,
    by_process: BTreeMap<Process, BTreeSet<RecordKey>>,
    open_by_process: BTreeMap<Process, BTreeSet<RecordKey>>,
    by_connection: BTreeMap<Connection, BTreeMap<Time, BTreeSet<RecordKey>>>,
    deadlines: BTreeSet<(Time, RecordKey)>,
    due_while_live: BTreeMap<Process, DueWhileLive>,
    witnesses: BTreeMap<Connection, BTreeSet<RecordKey>>,
    changed_tuples: BTreeSet<Connection>,
    // Each record process pins its ancestry and can be found from an owner
    // whose attribution changes. Live processes and recent events pin separately.
    paths: BTreeMap<Process, Vec<Process>>,
    dependents: BTreeMap<Process, BTreeSet<Process>>,
    live_ancestry: BTreeSet<Process>,
    garbage: BTreeSet<Process>,
    recent: BTreeMap<Process, f64>,
    exited: BTreeMap<Process, f64>,
    attribution: BTreeMap<Process, Attribution>,
    unattributed_changes: BTreeSet<Process>,
}
impl Default for Tracker {
    fn default() -> Self {
        Self::restore(Evidence::default())
    }
}
impl Tracker {
    pub fn restore(evidence: Evidence) -> Self {
        let garbage = evidence
            .parents
            .keys()
            .chain(evidence.origins.keys())
            .chain(evidence.shared.keys())
            .cloned()
            .collect();
        let mut tracker = Self {
            retention_seconds: DEFAULT_RETENTION_SECONDS,
            evidence,
            by_process: BTreeMap::new(),
            open_by_process: BTreeMap::new(),
            by_connection: BTreeMap::new(),
            deadlines: BTreeSet::new(),
            due_while_live: BTreeMap::new(),
            witnesses: BTreeMap::new(),
            changed_tuples: BTreeSet::new(),
            paths: BTreeMap::new(),
            dependents: BTreeMap::new(),
            live_ancestry: BTreeSet::new(),
            garbage,
            recent: BTreeMap::new(),
            exited: BTreeMap::new(),
            attribution: BTreeMap::new(),
            unattributed_changes: BTreeSet::new(),
        };
        let records = tracker.evidence.records.clone();
        for (key, row) in &records {
            tracker.index(key, row);
            // An outage can hide a close, fork, or multiplex listener.
            if row.record.closed_at.is_none() && !row.record.shared {
                tracker.edit_record(key).record.shared = true;
            }
        }
        tracker
    }

    pub fn evidence(&self) -> Evidence {
        self.evidence.clone()
    }

    pub fn records(&self) -> Records {
        self.evidence.records.clone()
    }

    fn edit_record(&mut self, key: &RecordKey) -> &mut StoredRecord {
        Arc::make_mut(self.evidence.records.get_mut(key).unwrap())
    }

    fn index(&mut self, key: &RecordKey, row: &StoredRecord) {
        if self
            .by_process
            .entry(key.0.clone())
            .or_default()
            .insert(key.clone())
            && !self.paths.contains_key(&key.0)
        {
            self.add_path(&key.0);
        }
        if row.record.closed_at.is_none() {
            self.open_by_process
                .entry(key.0.clone())
                .or_default()
                .insert(key.clone());
        }
        self.by_connection
            .entry(key.1.clone())
            .or_default()
            .entry(Time(row.record.opened_at))
            .or_default()
            .insert(key.clone());
        self.deadlines.insert((row.retention_start(), key.clone()));
    }

    fn add_path(&mut self, root: &Process) {
        let mut path = Vec::new();
        let mut current = root;
        let mut seen = BTreeSet::new();
        while seen.insert(current) {
            path.push(current.clone());
            self.dependents
                .entry(current.clone())
                .or_default()
                .insert(root.clone());
            let Some(parent) = self.evidence.parents.get(current) else {
                break;
            };
            current = parent;
        }
        self.paths.insert(root.clone(), path);
        self.unattributed_changes.insert(root.clone());
    }

    fn remove_path(&mut self, root: &Process) {
        let Some(path) = self.paths.remove(root) else {
            return;
        };
        for ancestor in path {
            remove_index(&mut self.dependents, &ancestor, root);
            self.garbage.insert(ancestor);
        }
    }

    fn set_parent(&mut self, child: Process, parent: Process) {
        if self.evidence.parents.get(&child) == Some(&parent) {
            return;
        }
        let affected = self.dependents.get(&child).cloned().unwrap_or_default();
        self.garbage.insert(child.clone());
        self.evidence.parents.insert(child, parent);
        for root in affected {
            self.remove_path(&root);
            self.add_path(&root);
        }
    }

    pub fn fork(&mut self, parent: &Process, child: &Process, at: f64) {
        self.recent.insert(child.clone(), at);
        self.set_parent(child.clone(), parent.clone());
        let keys = self
            .open_by_process
            .get(parent)
            .cloned()
            .unwrap_or_default();
        // Only connections already open could have been inherited at the fork.
        for key in keys {
            if !self.evidence.records[&key].record.shared {
                self.edit_record(&key).record.shared = true;
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
        self.garbage.insert(process.clone());
        self.evidence
            .origins
            .insert(process, Origin { connection, at });
    }

    pub fn opened(&mut self, process: Process, connection: Connection, at: f64, ssh: bool) {
        // Other protocols only matter as evidence that an old SSH tuple was reused.
        if !ssh && !self.by_connection.contains_key(&connection) {
            return;
        }
        self.recent.insert(process.clone(), at);
        let record = ConnectionRecord {
            closed_at: self.exited.get(&process).copied(),
            shared: self.evidence.shared.contains_key(&process),
            process,
            connection,
            opened_at: at,
            ssh,
            session: None,
            launch_chain: Vec::new(),
        };
        let key = record.key();
        if self.evidence.records.contains_key(&key) {
            self.observe_live(&key, at);
            return;
        }
        let last_seen = record.closed_at.is_none().then_some(at);
        let row = Arc::new(StoredRecord { record, last_seen });
        self.index(&key, &row);
        self.changed_tuples.insert(key.1.clone());
        self.unattributed_changes.insert(key.0.clone());
        self.evidence.records.insert(key, row);
    }

    fn close_record(&mut self, key: &RecordKey, at: f64) {
        let start = self.evidence.records[key].retention_start();
        self.deadlines.remove(&(start, key.clone()));
        let row = self.edit_record(key);
        row.record.closed_at = Some(at);
        row.last_seen = None;
        self.deadlines.insert((Time(at), key.clone()));
        self.changed_tuples.insert(key.1.clone());
        remove_index(&mut self.open_by_process, &key.0, key);
    }

    pub fn closed(&mut self, process: &Process, connection: &Connection, at: f64) {
        if let Some(key) = self
            .open_by_process
            .get(process)
            .into_iter()
            .flatten()
            .rev()
            .find(|key| key.1 == *connection && self.evidence.records[*key].record.opened_at <= at)
            .cloned()
        {
            self.close_record(&key, at);
        }
    }

    pub fn exit(&mut self, process: &Process, at: f64) {
        self.exited.insert(process.clone(), at);
        self.garbage.insert(process.clone());
        let keys = self
            .open_by_process
            .get(process)
            .cloned()
            .unwrap_or_default();
        for key in keys {
            self.close_record(&key, at);
        }
    }

    pub fn shared(&mut self, process: &Process) {
        if self.evidence.shared.contains_key(process) {
            return;
        }
        self.evidence.shared.insert(process.clone(), ());
        self.garbage.insert(process.clone());
        let keys = self.by_process.get(process).cloned().unwrap_or_default();
        for key in keys {
            if !self.evidence.records[&key].record.shared {
                self.edit_record(&key).record.shared = true;
            }
        }
    }

    pub fn gap(&mut self) {
        // Event loss may hide a listener or fork anywhere in the evidence.
        let changed: Vec<_> = self
            .evidence
            .records
            .iter()
            .filter(|(_, row)| !row.record.shared)
            .map(|(key, _)| key.clone())
            .collect();
        for key in changed {
            self.edit_record(&key).record.shared = true;
        }
    }

    pub fn origin(&self, process: &Process) -> Option<&Origin> {
        let mut current = process;
        let mut seen = BTreeSet::new();
        while seen.insert(current) {
            if let Some(origin) = self.evidence.origins.get(current) {
                return Some(origin);
            }
            current = self.evidence.parents.get(current)?;
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
        let mut current = self.evidence.parents.get(process)?;
        let mut seen = BTreeSet::new();
        while seen.insert(current) {
            if self.evidence.shared.contains_key(current)
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
            current = self.evidence.parents.get(current)?;
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
                && !self.evidence.parents.contains_key(&entry.process)
            {
                self.set_parent(entry.process.clone(), parent.process.clone());
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
            .map(|owner| (&owner.process, &owner.session))
            .collect();
        let current: BTreeMap<_, _> = bindings
            .keys()
            .chain(owners.keys().copied())
            .filter(|process| self.dependents.contains_key(*process))
            .map(|process| {
                let binding = bindings.get(process);
                let session = owners
                    .get(process)
                    .copied()
                    .or_else(|| binding.map(|b| &b.session))
                    .unwrap();
                (
                    process.clone(),
                    Attribution {
                        session: session.clone(),
                        binding: binding.map(|b| (b.session.clone(), b.launch_chain.clone())),
                    },
                )
            })
            .collect();
        let mut changed = std::mem::take(&mut self.unattributed_changes);
        for (process, previous) in &self.attribution {
            if current.get(process) != Some(previous) {
                changed.extend(self.dependents.get(process).into_iter().flatten().cloned());
            }
        }
        for process in current.keys() {
            if !self.attribution.contains_key(process) {
                changed.extend(self.dependents[process].iter().cloned());
            }
        }
        self.attribution = current;
        for process in changed {
            self.attribute_process(&process, bindings, &owners);
        }
    }

    pub fn attribute_exiting(
        &mut self,
        process: &Process,
        bindings: &BTreeMap<Process, Binding>,
        catalog: &Catalog,
    ) {
        if !self.by_process.contains_key(process) {
            return;
        }
        let owners = catalog
            .owners
            .iter()
            .map(|o| (&o.process, &o.session))
            .collect();
        self.attribute_process(process, bindings, &owners);
    }

    fn attribute_process(
        &mut self,
        process: &Process,
        bindings: &BTreeMap<Process, Binding>,
        owners: &BTreeMap<&Process, &Session>,
    ) {
        let Some(keys) = self.by_process.get(process) else {
            return;
        };
        // Resolve once for all connections of this incarnation.
        let mut current = process;
        let mut seen = BTreeSet::new();
        while seen.insert(current) {
            let binding = bindings.get(current);
            if let Some(owner) = owners
                .get(current)
                .copied()
                .or_else(|| binding.map(|b| &b.session))
            {
                for key in keys {
                    let row = &self.evidence.records[key];
                    if row.record.session.is_none()
                        || binding.is_some_and(|b| {
                            same_session(row.record.session.as_ref(), &b.session)
                                && b.launch_chain.iter().any(|launch| {
                                    !row.record
                                        .launch_chain
                                        .iter()
                                        .any(|old| old.process == launch.process)
                                })
                        })
                    {
                        let record =
                            &mut Arc::make_mut(self.evidence.records.get_mut(key).unwrap()).record;
                        record.session.get_or_insert_with(|| owner.clone());
                        if let Some(binding) = binding {
                            enrich(record, binding);
                        }
                    }
                }
                break;
            }
            let Some(parent) = self.evidence.parents.get(current) else {
                break;
            };
            current = parent;
        }
    }

    fn observe_live(&mut self, key: &RecordKey, at: f64) {
        let row = &self.evidence.records[key];
        if row.record.closed_at.is_some() || row.last_seen == Some(at) {
            return;
        }
        let start = row.retention_start();
        self.deadlines.remove(&(start, key.clone()));
        let row = self.edit_record(key);
        row.last_seen = Some(at);
        let start = row.retention_start();
        self.deadlines.insert((start, key.clone()));
    }

    fn remove_record(&mut self, key: &RecordKey) {
        let row = self.evidence.records.remove(key).unwrap();
        self.deadlines.remove(&(row.retention_start(), key.clone()));
        remove_index(&mut self.by_process, &key.0, key);
        remove_index(&mut self.open_by_process, &key.0, key);
        let times = self.by_connection.get_mut(&key.1).unwrap();
        remove_index(times, &Time(row.record.opened_at), key);
        if times.is_empty() {
            self.by_connection.remove(&key.1);
        }
        if !self.by_process.contains_key(&key.0) {
            self.remove_path(&key.0);
            self.unattributed_changes.remove(&key.0);
        }
    }

    fn prune(&mut self, snapshot: &Snapshot, at: f64) {
        let cutoff = at - self.retention_seconds - 2.0;
        // Only unfinished observations need a durable last-live timestamp.
        for entry in snapshot.entries.values() {
            let Some(open) = self.open_by_process.get(&entry.process) else {
                continue;
            };
            let keys: Vec<_> = if entry.socket_scan_complete {
                entry
                    .sockets
                    .iter()
                    .flat_map(|tuple| {
                        open.range(
                            (entry.process.clone(), tuple.clone(), 0)
                                ..=(entry.process.clone(), tuple.clone(), u64::MAX),
                        )
                        .cloned()
                    })
                    .collect()
            } else {
                open.iter().cloned().collect()
            };
            for key in keys {
                if self.evidence.records[&key].retention_start().0 < cutoff {
                    self.changed_tuples.insert(key.1.clone());
                }
                self.observe_live(&key, at);
            }
        }

        // Closed evidence that is past its deadline can still be live. Recheck
        // it only when the incarnation or its socket coverage changes.
        self.due_while_live.retain(|process, due| {
            if let Some(entry) = observed_process(snapshot, process)
                && (!entry.socket_scan_complete && due.sockets.is_none()
                    || entry.socket_scan_complete && due.sockets.as_ref() == Some(&entry.sockets))
            {
                return true;
            }
            for key in &due.keys {
                if let Some(row) = self.evidence.records.get(key) {
                    self.deadlines.insert((row.retention_start(), key.clone()));
                }
            }
            false
        });

        let mut expired = BTreeMap::<Connection, BTreeSet<RecordKey>>::new();
        while let Some((time, _)) = self.deadlines.first() {
            if time.0 >= cutoff {
                break;
            }
            let (_, key) = self.deadlines.pop_first().unwrap();
            if let Some(entry) = observed_process(snapshot, &key.0)
                && (!entry.socket_scan_complete || entry.sockets.contains(&key.1))
            {
                self.due_while_live
                    .entry(key.0.clone())
                    .or_insert_with(|| DueWhileLive {
                        sockets: entry.socket_scan_complete.then(|| entry.sockets.clone()),
                        keys: BTreeSet::new(),
                    })
                    .keys
                    .insert(key);
            } else {
                expired.entry(key.1.clone()).or_default().insert(key);
            }
        }

        let mut changed = std::mem::take(&mut self.changed_tuples);
        changed.extend(expired.keys().cloned());
        for tuple in changed {
            let mut candidates = expired.remove(&tuple).unwrap_or_default();
            if let Some(witnesses) = self.witnesses.remove(&tuple) {
                candidates.extend(witnesses);
            }
            let mut remove = Vec::new();
            for key in candidates {
                let row = &self.evidence.records[&key];
                if !is_expired(row, snapshot, cutoff) {
                    // It may be live now. Keep a deadline if it was parked as a witness.
                    self.deadlines.insert((row.retention_start(), key));
                    continue;
                }
                let opened = Time(row.record.opened_at);
                let times = &self.by_connection[&tuple];
                let witness = times[&opened].first() == Some(&key)
                    && times
                        .range((Unbounded, Excluded(opened)))
                        .next_back()
                        .is_some_and(|(_, earlier)| {
                            earlier.iter().any(|candidate| {
                                let row = &self.evidence.records[candidate];
                                row.record.closed_at.is_none() && !is_expired(row, snapshot, cutoff)
                            })
                        });
                if witness {
                    self.witnesses.entry(tuple.clone()).or_default().insert(key);
                } else {
                    remove.push(key);
                }
            }
            // Decide all witnesses against the same pre-removal tuple history.
            for key in remove {
                self.remove_record(&key);
            }
        }

        self.recent.retain(|_, seen| *seen + 2.0 >= at);
        let mut live = BTreeSet::new();
        for process in snapshot
            .entries
            .values()
            .map(|e| &e.process)
            .chain(self.recent.keys())
        {
            let mut current = process;
            while live.insert(current.clone()) {
                let Some(parent) = self.evidence.parents.get(current) else {
                    break;
                };
                current = parent;
            }
        }
        self.garbage
            .extend(self.live_ancestry.difference(&live).cloned());
        self.live_ancestry = live;
        for process in std::mem::take(&mut self.garbage) {
            if !self.live_ancestry.contains(&process) && !self.dependents.contains_key(&process) {
                self.evidence.parents.remove(&process);
                self.evidence.origins.remove(&process);
                self.evidence.shared.remove(&process);
                self.exited.remove(&process);
            }
        }
    }
}

fn remove_index<K: Ord, V: Ord>(index: &mut BTreeMap<K, BTreeSet<V>>, key: &K, value: &V) {
    if let Some(values) = index.get_mut(key) {
        values.remove(value);
        if values.is_empty() {
            index.remove(key);
        }
    }
}

fn observed_process<'a>(
    snapshot: &'a Snapshot,
    process: &Process,
) -> Option<&'a crate::linux::Entry> {
    snapshot
        .entries
        .get(&process.pid)
        .filter(|entry| entry.process == *process)
}

fn is_expired(row: &StoredRecord, snapshot: &Snapshot, cutoff: f64) -> bool {
    row.retention_start().0 < cutoff
        && !observed_process(snapshot, &row.record.process).is_some_and(|entry| {
            !entry.socket_scan_complete || entry.sockets.contains(&row.record.connection)
        })
}

fn same_session(session: Option<&Session>, other: &Session) -> bool {
    session.is_some_and(|session| {
        session.node_id == other.node_id
            && session.source == other.source
            && session.sid == other.sid
    })
}

fn enrich(record: &mut ConnectionRecord, binding: &Binding) {
    if !same_session(record.session.as_ref(), &binding.session) {
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
