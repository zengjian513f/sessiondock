//! Agy's SQLite step metadata identifies the current use of a step index.

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

fn varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut value = 0;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = bytes.split_first()?;
        *bytes = rest;
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Some(value);
        }
    }
    None
}

pub(crate) fn bytes_field(mut bytes: &[u8], wanted: u64) -> Option<&[u8]> {
    while !bytes.is_empty() {
        let tag = varint(&mut bytes)?;
        let length = match tag & 7 {
            0 => {
                varint(&mut bytes)?;
                continue;
            }
            1 => 8,
            2 => usize::try_from(varint(&mut bytes)?).ok()?,
            5 => 4,
            _ => return None,
        };
        let value = bytes.get(..length)?;
        bytes = bytes.get(length..)?;
        if tag >> 3 == wanted && tag & 7 == 2 {
            return Some(value);
        }
    }
    None
}

fn created_second(metadata: Option<&[u8]>) -> Option<i64> {
    // Cascade step metadata field 1 is created_at (google.protobuf.Timestamp).
    // Timestamp seconds are field 1. Omitted seconds represent the epoch.
    let mut timestamp = bytes_field(metadata?, 1)?;
    while !timestamp.is_empty() {
        match varint(&mut timestamp)? {
            8 => return Some(varint(&mut timestamp)? as i64),
            16 => {
                varint(&mut timestamp)?;
            }
            _ => return None,
        }
    }
    Some(0)
}

pub(crate) fn record_created_second(record: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(record["created_at"].as_str()?)
        .ok()
        .map(|time| time.timestamp())
}

#[derive(Default)]
pub(crate) struct InputRevisions(BTreeMap<i64, (i64, i64)>);

impl InputRevisions {
    pub(crate) fn observe(&mut self, index: i64, created: i64) {
        let (first, last) = self.0.entry(index).or_insert((created, created));
        *first = (*first).min(created);
        *last = (*last).max(created);
    }

    pub(crate) fn replacements(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.0
            .iter()
            .filter_map(|(&index, &(first, last))| (first < last).then_some((index, last)))
    }

    pub(crate) fn rewinds(&self, is_current: impl Fn(i64, i64) -> bool) -> BTreeMap<i64, i64> {
        let mut boundaries = BTreeMap::new();
        let mut newest = i64::MIN;
        for (index, created) in self.replacements() {
            if created > newest && is_current(index, created) {
                boundaries.insert(index, created);
                newest = created;
            }
        }
        boundaries
    }
}

pub(super) fn current(path: &Path) -> rusqlite::Result<BTreeMap<i64, Option<i64>>> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut query = db
        .prepare("SELECT idx, metadata FROM steps")
        .or_else(|_| db.prepare("SELECT idx, NULL FROM steps"))?;
    query
        .query_map([], |row| {
            Ok((row.get(0)?, created_second(row.get_ref(1)?.as_blob().ok())))
        })?
        .collect()
}

pub(crate) fn created_seconds(
    path: &Path,
    indices: Vec<i64>,
) -> rusqlite::Result<BTreeMap<i64, i64>> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut query = db.prepare("SELECT metadata FROM steps WHERE idx = ?1")?;
    let mut times = BTreeMap::new();
    for index in indices {
        if let Some(Some(time)) = query
            .query_row([index], |row| {
                Ok(created_second(row.get_ref(0)?.as_blob().ok()))
            })
            .optional()?
        {
            times.insert(index, time);
        }
    }
    Ok(times)
}
