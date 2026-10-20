//! Optional connection-history transport; correlation always sees a full report.
use process_links::{Connection, Process, Report, connections::ConnectionRecord};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
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
    pub report: Report,
    pub remove: Vec<Key>,
}

struct Snapshot {
    revision: String,
    node_id: String,
    boot_id: String,
    records: BTreeMap<Key, Arc<ConnectionRecord>>,
}

#[derive(Default)]
pub struct Store(Mutex<VecDeque<Snapshot>>);

impl Store {
    /// Only collector reports participate. A temporary polling fallback must
    /// not evict the retained collector baseline and retransmit it on recovery.
    pub fn update(&self, mut report: Report, since: Option<&str>) -> Result<Update, Box<Report>> {
        if report.collector.is_none() {
            return Err(Box::new(report));
        }
        let mut snapshots = self.0.lock().unwrap_or_else(|e| e.into_inner());
        snapshots.retain(|s| s.node_id == report.node_id && s.boot_id == report.boot_id);
        let previous = snapshots.back();
        let mut records = BTreeMap::new();
        for record in &report.connections {
            let id = key(record);
            let shared = previous
                .and_then(|s| s.records.get(&id))
                .filter(|old| old.as_ref() == record)
                .cloned()
                .unwrap_or_else(|| Arc::new(record.clone()));
            // Malformed/older collector duplicates still receive the original
            // full report; transport must never silently discard evidence.
            if records.insert(id, shared).is_some() {
                return Err(Box::new(report));
            }
        }
        let unchanged = previous.is_some_and(|s| s.records == records);
        let revision = if unchanged {
            previous.unwrap().revision.clone()
        } else {
            let mut random = [0u8; 16];
            if getrandom::fill(&mut random).is_err() {
                return Err(Box::new(report));
            }
            random.iter().map(|byte| format!("{byte:02x}")).collect()
        };
        let baseline = since.and_then(|revision| snapshots.iter().find(|s| s.revision == revision));
        let base = baseline.map(|s| s.revision.clone());
        let mut remove = Vec::new();
        if let Some(baseline) = baseline {
            report.connections.retain(|record| {
                baseline.records.get(&key(record)).map(Arc::as_ref) != Some(record)
            });
            remove.extend(
                baseline
                    .records
                    .keys()
                    .filter(|id| !records.contains_key(*id))
                    .cloned(),
            );
        }
        if !unchanged {
            snapshots.push_back(Snapshot {
                revision: revision.clone(),
                node_id: report.node_id.clone(),
                boot_id: report.boot_id.clone(),
                records,
            });
            // Shared unchanged records keep two revisions cheap. Eviction only
            // causes a full snapshot; no history is removed from the collector.
            while snapshots.len() > 2 {
                snapshots.pop_front();
            }
        }
        Ok(Update {
            revision,
            base,
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
