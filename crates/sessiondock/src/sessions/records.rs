//! Bounded, disposable JSON AST reuse. Never an incremental timeline parser.
//! Ownership moves out of the cache while projecting, avoiding deep AST clones
//! and preventing old ViewSnapshot Arcs from pinning cache entries indefinitely.
use super::{Candidate, Parsed, SessionError, budgets, native_input::RawIndex, uid_for};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) mod native_images;
mod native_records;
pub(crate) mod scanner;
pub(crate) mod string_reader;
mod tool_envelopes;
pub(super) use native_records::scan_native_records;
#[cfg(test)]
mod scanner_contract_tests;

/// Runtime AST budget: `SESSIONDOCK_AST_CACHE_MB`.
fn max_weight() -> usize {
    budgets::caches().ast_bytes
}
fn max_entries() -> usize {
    budgets::caches().ast_entries
}
/// Strings longer than this become private spans during the structural scan.
const SPAN_THRESHOLD: usize = budgets::INLINE_STRING_BYTES;

/// Structural limits shared by the small-record and streaming record paths.
fn record_limits() -> scanner::Limits {
    scanner::Limits::native(SPAN_THRESHOLD)
}

/// Strict small-record adapter. The pull decoder separately handles private
/// large spans; never replace them with public markers or empty strings here.
pub(super) fn decode_record(line: &[u8]) -> Result<Value, scanner::ScanError> {
    scanner::scan_value(line, record_limits())
}

/// A whole resident line (push path: tests and the empty Grok chat) has no
/// file to read spans back from; all strings from the actual line stay inline.
fn decode_resident_record(line: &[u8]) -> Result<Value, scanner::ScanError> {
    scanner::scan_value(
        line,
        scanner::Limits {
            inline_string_bytes: line.len(),
        },
    )
}

struct Entry {
    candidate: Candidate,
    committed: usize,
    records: Vec<(Value, u64)>,
    sidecars: BTreeMap<u64, Vec<native_images::Sidecar>>,
    invalid: usize,
    weight: usize,
    used: u64,
}
pub(crate) struct RecordCache {
    entries: BTreeMap<String, Entry>,
    weight: usize,
    clock: u64,
    /// This cache's retention budget; oversized parsed records stay readable.
    max_weight: usize,
    max_entries: usize,
    #[cfg(test)]
    pub decoded: usize,
    #[cfg(test)]
    pub reused: usize,
}
impl Default for RecordCache {
    fn default() -> Self {
        Self::with_budget(max_weight(), max_entries())
    }
}
impl RecordCache {
    pub(crate) fn with_budget(max_weight: usize, max_entries: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            weight: 0,
            clock: 0,
            max_weight,
            max_entries,
            #[cfg(test)]
            decoded: 0,
            #[cfg(test)]
            reused: 0,
        }
    }
}
pub(crate) struct Batch {
    pub records: Vec<(Value, u64)>,
    pub sidecars: BTreeMap<u64, Vec<native_images::Sidecar>>,
    pub error: Option<String>,
    /// Complete lines that are not JSON objects, skipped
    /// adapters' `_iter_records` (bytes stay in the physical index).
    pub invalid: usize,
    committed: usize,
    weight: usize,
}

/// The non-fatal `migration_warnings` line for `count` skipped lines.
pub(crate) fn invalid_lines_warning(count: usize) -> Option<String> {
    (count > 0).then(|| format!("跳过无效的JSONL 记录 ×{count}"))
}
impl RecordCache {
    /// All raw bytes pass the index, even when prior ASTs are candidates for
    /// reuse. A full SHA-256 prefix match is required before exposing reused
    /// records. On mismatch, reopen the same stamped input for a cold pass.
    pub(super) fn decode_input(
        &mut self,
        candidate: &Candidate,
        previous: Option<&Parsed>,
        mut scan: impl FnMut(&mut Decoder, Option<u64>) -> Result<RawIndex, SessionError>,
    ) -> Result<(Batch, RawIndex), SessionError> {
        let key = uid_for(candidate.source, &candidate.path);
        let prior = self.entries.remove(&key);
        if let Some(entry) = &prior {
            self.weight -= entry.weight;
        }
        let prior = prior.filter(|entry| {
            previous.is_some_and(|previous| {
                previous.raw_error.is_none()
                    && entry.candidate == previous.candidate
                    && entry.committed == previous.committed
                    && same_file(&entry.candidate, candidate)
                    && candidate
                        .data_stamp()
                        .is_some_and(|stamp| stamp.size >= entry.committed as u64)
            })
        });
        let probe = prior.as_ref().map(|entry| entry.committed as u64);
        let expected = probe.map(|_| {
            previous
                .expect("checked prior")
                .raw_index
                .committed_digest()
        });
        let reused = prior.as_ref().map_or(0, |entry| entry.records.len());
        let mut decoder = Decoder::new(prior);
        let mut index = scan(&mut decoder, probe)?;
        #[cfg(test)]
        let speculative_decoded = decoder.decoded;
        let matched = expected.is_none_or(|expected| index.probe_digest() == Some(expected));
        if !matched {
            // Drop stale ASTs/index before allocating their replacement. Never
            // let a failed speculative prefix prove any old event authority.
            drop(decoder);
            drop(index);
            decoder = Decoder::new(None);
            index = scan(&mut decoder, None)?;
        }
        #[cfg(test)]
        {
            self.decoded += decoder.decoded + if matched { 0 } else { speculative_decoded };
            self.reused += if matched { reused } else { 0 };
        }
        #[cfg(not(test))]
        let _ = reused;
        let batch = decoder.finish(index.committed() as usize);
        Ok((batch, index))
    }

    #[cfg(test)]
    pub fn decode(
        &mut self,
        candidate: &Candidate,
        previous: Option<&Parsed>,
        bytes: &[u8],
        committed: usize,
    ) -> Batch {
        let (batch, _) = self
            .decode_input(candidate, previous, |decoder, probe| {
                scan_records(bytes, decoder, probe)
            })
            .unwrap();
        assert_eq!(batch.committed, committed);
        batch
    }
    pub fn retain(&mut self, candidate: Candidate, batch: Batch) {
        let (max_weight, max_entries) = (self.max_weight, self.max_entries);
        if batch.error.is_some() || batch.weight > max_weight || max_entries == 0 {
            return;
        }
        while self.entries.len() >= max_entries
            || self.weight.saturating_add(batch.weight) > max_weight
        {
            let Some(key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            let entry = self.entries.remove(&key).expect("selected cache entry");
            self.weight -= entry.weight;
        }
        if self.clock == u64::MAX {
            let mut order = self
                .entries
                .iter()
                .map(|(key, entry)| (entry.used, key.clone()))
                .collect::<Vec<_>>();
            order.sort();
            for (index, (_, key)) in order.into_iter().enumerate() {
                self.entries.get_mut(&key).unwrap().used = index as u64;
            }
            self.clock = self.entries.len() as u64;
        }
        self.clock += 1;
        let key = uid_for(candidate.source, &candidate.path);
        self.weight += batch.weight;
        if let Some(old) = self.entries.insert(
            key,
            Entry {
                candidate,
                committed: batch.committed,
                records: batch.records,
                sidecars: batch.sidecars,
                invalid: batch.invalid,
                weight: batch.weight,
                used: self.clock,
            },
        ) {
            self.weight -= old.weight;
        }
    }
}

/// One bounded complete record at a time. An oversized partial tail is scanned
/// and hashed but does not fail history until its LF commits it. A complete
/// line that is not a JSON object is skipped and counted (`invalid`);
/// only the record budgets are hard errors.
pub(super) struct Decoder {
    records: Vec<(Value, u64)>,
    sidecars: BTreeMap<u64, Vec<native_images::Sidecar>>,
    weight: usize,
    skip: u64,
    offset: u64,
    line: Vec<u8>,
    invalid: usize,
    error: Option<String>,
    #[cfg(test)]
    decoded: usize,
}
impl Decoder {
    fn new(prior: Option<Entry>) -> Self {
        let (records, sidecars, skip, invalid, weight) = prior.map_or_else(
            || (Vec::new(), BTreeMap::new(), 0, 0, 0),
            |entry| {
                (
                    entry.records,
                    entry.sidecars,
                    entry.committed as u64,
                    entry.invalid,
                    entry.weight,
                )
            },
        );
        Self {
            records,
            sidecars,
            weight,
            skip,
            offset: 0,
            line: Vec::new(),
            invalid,
            error: None,
            #[cfg(test)]
            decoded: 0,
        }
    }
    pub(super) fn cold() -> Self {
        Self::new(None)
    }
    fn feed(&mut self, mut bytes: &[u8]) {
        if self.offset < self.skip {
            let count = bytes.len().min((self.skip - self.offset) as usize);
            self.offset += count as u64;
            bytes = &bytes[count..];
        }
        for piece in bytes.split_inclusive(|byte| *byte == b'\n') {
            self.offset += piece.len() as u64;
            if self.error.is_some() {
                continue;
            }
            let complete = piece.last() == Some(&b'\n');
            if self.line.try_reserve(piece.len()).is_err() {
                self.error = Some("原生记录缓冲区分配失败".into());
                continue;
            }
            self.line.extend_from_slice(piece);
            if !complete {
                continue;
            }
            if !self.line.iter().all(u8::is_ascii_whitespace) {
                #[cfg(test)]
                {
                    self.decoded += 1;
                }
                match decode_resident_record(&self.line) {
                    Ok(row) if row.is_object() => {
                        self.weight = self
                            .weight
                            .saturating_add(value_weight(&row))
                            .saturating_add(32);
                        self.records.push((row, self.offset));
                    }
                    _ => self.invalid += 1,
                }
            }
            self.line.clear();
        }
    }
    pub(super) fn finish(self, committed: usize) -> Batch {
        Batch {
            records: self.records,
            sidecars: self.sidecars,
            weight: self.weight,
            error: self.error,
            invalid: self.invalid,
            committed,
        }
    }
}

/// Index and decode consume the exact same stream; no complete raw body is
/// retained, and failures leave the caller responsible for rejecting the scan.
pub(super) fn scan_records(
    reader: impl std::io::Read,
    decoder: &mut Decoder,
    probe: Option<u64>,
) -> Result<RawIndex, SessionError> {
    struct Tee<'a, R> {
        reader: R,
        decoder: &'a mut Decoder,
    }
    impl<R: std::io::Read> std::io::Read for Tee<'_, R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.reader.read(buffer)?;
            self.decoder.feed(&buffer[..count]);
            Ok(count)
        }
    }
    let tee = Tee { reader, decoder };
    match probe {
        Some(end) => RawIndex::scan_with_probe(tee, Some(end)),
        None => RawIndex::scan(tee),
    }
}
fn same_file(old: &Candidate, new: &Candidate) -> bool {
    old.source == new.source
        && old.root == new.root
        && old.path == new.path
        && old.data == new.data
        && old.summary == new.summary
        && old.data_stamp().map(|stamp| &stamp.file_identity)
            == new.data_stamp().map(|stamp| &stamp.file_identity)
}
// A conservative logical weight, not an allocator/RSS assertion. Containers pay
// per capacity/entry and each nested Value pays a base node charge; an AST of
// millions of tiny scalars cannot be retained merely because its JSON is small.
fn value_weight(value: &Value) -> usize {
    let nested = match value {
        Value::String(text) => text.capacity(),
        Value::Array(items) => items
            .iter()
            .fold(items.capacity().saturating_mul(32), |total, item| {
                total.saturating_add(value_weight(item))
            }),
        Value::Object(items) => items.iter().fold(0usize, |total, (key, value)| {
            total
                .saturating_add(128)
                .saturating_add(key.capacity())
                .saturating_add(value_weight(value))
        }),
        _ => 0,
    };
    64usize.saturating_add(nested)
}

#[cfg(test)]
mod tests;
