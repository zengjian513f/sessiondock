//! Optional connection-history transport; correlation always sees a full report.
use process_links::{Connection, Process, Report, connections::ConnectionRecord};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub const HEADER: &str = "x-sessiondock-connections";
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key(Process, Connection, u64);

// Send the timestamp in exactly the same JSON representation as opened_at.
// Its decoded bits must agree with the decoded record even when a JSON parser
// rounds a fractional timestamp; sending the sender's raw bits would not.
impl Serialize for Key {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.0, &self.1, f64::from_bits(self.2)).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (process, connection, opened_at) =
            <(Process, Connection, f64)>::deserialize(deserializer)?;
        Ok(Self(process, connection, opened_at.to_bits()))
    }
}

pub fn key(record: &ConnectionRecord) -> Key {
    Key(
        record.process.clone(),
        record.connection.clone(),
        record.opened_at.to_bits(),
    )
}

#[derive(Serialize, Deserialize)]
pub struct Update {
    pub revision: String,
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_reason: Option<String>,
    pub report: Report,
    pub remove: Vec<Key>,
}

struct Record {
    changed_at: u64,
    value: Arc<ConnectionRecord>,
}

#[derive(Default)]
struct Journal {
    epoch: String,
    sequence: u64,
    node_id: String,
    boot_id: String,
    records: BTreeMap<Key, Record>,
    removed: BTreeMap<Key, u64>,
}

#[derive(Default)]
pub struct Store(Mutex<Journal>);

impl Store {
    /// Only collector reports participate. Keep each record's last change and
    /// deletion sequence until this listener/boot ends, so lost replies and
    /// other readers cannot evict an acknowledged cursor.
    pub fn update(&self, mut report: Report, since: Option<&str>) -> Result<Update, Box<Report>> {
        if report.collector.is_none() {
            return Err(Box::new(report));
        }
        let mut journal = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if journal.epoch.is_empty()
            || journal.node_id != report.node_id
            || journal.boot_id != report.boot_id
        {
            let mut random = [0u8; 8];
            if getrandom::fill(&mut random).is_err() {
                return Err(Box::new(report));
            }
            *journal = Journal {
                epoch: random.iter().map(|byte| format!("{byte:02x}")).collect(),
                node_id: report.node_id.clone(),
                boot_id: report.boot_id.clone(),
                ..Journal::default()
            };
        }
        // The wire cursor remains 32 hexadecimal characters for existing Hubs.
        // It names this journal and a sequence, not an evictable full snapshot.
        let baseline = since.and_then(|cursor| {
            cursor
                .strip_prefix(&journal.epoch)
                .filter(|sequence| sequence.len() == 16)
                .and_then(|sequence| u64::from_str_radix(sequence, 16).ok())
                .filter(|sequence| *sequence <= journal.sequence)
        });
        let next = journal.sequence + 1;
        let mut records = BTreeMap::new();
        let mut changed = false;
        for record in &report.connections {
            let id = key(record);
            let (changed_at, value) = match journal.records.get(&id) {
                Some(old) if old.value.as_ref() == record => (old.changed_at, old.value.clone()),
                _ => {
                    changed = true;
                    (next, Arc::new(record.clone()))
                }
            };
            // Preserve the ordinary full report for duplicate native identities;
            // never partially advance the journal or discard evidence.
            if records.insert(id, Record { changed_at, value }).is_some() {
                return Err(Box::new(report));
            }
        }
        let removed: Vec<_> = journal
            .records
            .keys()
            .filter(|id| !records.contains_key(*id))
            .cloned()
            .collect();
        changed |= !removed.is_empty();
        if changed {
            for id in removed {
                journal.removed.insert(id, next);
            }
            // A reappearing identity is covered by its new record; it no longer
            // needs a tombstone, even for readers that missed the deletion.
            journal.removed.retain(|id, _| !records.contains_key(id));
            journal.sequence = next;
            journal.records = records;
        }
        let mut remove = Vec::new();
        if let Some(baseline) = baseline {
            report
                .connections
                .retain(|record| journal.records[&key(record)].changed_at > baseline);
            remove.extend(
                journal
                    .removed
                    .iter()
                    .filter(|(_, sequence)| **sequence > baseline)
                    .map(|(id, _)| id.clone()),
            );
        }
        Ok(Update {
            revision: format!("{}{:016x}", journal.epoch, journal.sequence),
            base: baseline.and(since.map(str::to_owned)),
            reset_reason: baseline.is_none().then(|| {
                match since {
                    None => "initial",
                    Some(cursor) if !cursor.starts_with(&journal.epoch) => "journal_changed",
                    _ => "unknown_cursor",
                }
                .to_owned()
            }),
            report,
            remove,
        })
    }
}

pub struct Remote {
    pub revision: String,
    node_id: String,
    boot_id: String,
    records: BTreeMap<Key, ConnectionRecord>,
}

impl Remote {
    pub fn apply(baseline: &mut Option<Self>, update: Update) -> Option<Report> {
        let Update {
            revision,
            base,
            reset_reason: _,
            mut report,
            remove,
        } = update;
        if let Some(base) = base {
            let Some(cache) = baseline.as_mut().filter(|c| {
                c.revision == base && c.node_id == report.node_id && c.boot_id == report.boot_id
            }) else {
                *baseline = None;
                return None;
            };
            for id in remove {
                cache.records.remove(&id);
            }
            for record in report.connections {
                cache.records.insert(key(&record), record);
            }
            cache.revision = revision;
            report.connections = cache.records.values().cloned().collect();
        } else {
            let records: BTreeMap<_, _> = report
                .connections
                .iter()
                .map(|r| (key(r), r.clone()))
                .collect();
            if records.len() != report.connections.len() || !remove.is_empty() {
                *baseline = None;
                return None;
            }
            *baseline = Some(Self {
                revision,
                node_id: report.node_id.clone(),
                boot_id: report.boot_id.clone(),
                records,
            });
        }
        Some(report)
    }
}
