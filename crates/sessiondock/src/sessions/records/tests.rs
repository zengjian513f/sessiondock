const LINE_LIMIT: usize = 64 * 1024 * 1024;
const ROW_LIMIT: usize = 1_000_000;
use super::super::FileStamp;
use super::*;
use serde_json::json;
use std::path::PathBuf;

fn committed(bytes: &[u8]) -> usize {
    bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1)
}

fn candidate(id: usize, bytes: &[u8]) -> Candidate {
    let root = PathBuf::from("synthetic-record-root");
    let path = root.join(format!("record-{id}.jsonl"));
    Candidate {
        source: "claude",
        root,
        data: path.clone(),
        path,
        summary: None,
        stamps: vec![FileStamp {
            size: bytes.len() as u64,
            modified: bytes.len() as u128,
            identity: format!("synthetic-stat-{id}-{}", bytes.len()),
            file_identity: format!("synthetic-inode-{id}"),
        }],
    }
}

fn parsed(candidate: Candidate, bytes: &[u8], raw_error: Option<String>) -> Parsed {
    Parsed {
        candidate,
        _fixture: None,
        raw_index: super::super::native_input::RawIndex::scan(bytes).unwrap(),
        committed: committed(bytes),
        meta: json!({}),
        native_id: Ok("synthetic-native-id".into()),
        events: Vec::new(),
        unsupported: raw_error.clone(),
        raw_error,
        semantic_digest: "synthetic-projection".into(),
        pin: None,
    }
}

fn seed(cache: &mut RecordCache, id: usize, bytes: &[u8]) -> Parsed {
    let candidate = candidate(id, bytes);
    let batch = cache.decode(&candidate, None, bytes, committed(bytes));
    assert!(batch.error.is_none());
    cache.retain(candidate.clone(), batch);
    parsed(candidate, bytes, None)
}

fn assert_matches_cold(candidate: &Candidate, bytes: &[u8], batch: &Batch) {
    let cold = RecordCache::default().decode(candidate, None, bytes, committed(bytes));
    assert_eq!(batch.records, cold.records);
    assert_eq!(batch.error, cold.error);
    assert_eq!(batch.invalid, cold.invalid);
    assert_eq!(batch.committed, cold.committed);
    assert_eq!(batch.weight, cold.weight);
}

fn assert_accounting(cache: &RecordCache) {
    assert_eq!(
        cache.weight,
        cache
            .entries
            .values()
            .map(|entry| entry.weight)
            .sum::<usize>()
    );
    assert!(cache.weight <= cache.max_weight);
    assert!(cache.entries.len() <= cache.max_entries);
}

#[test]
fn append_decodes_only_new_rows_and_unchanged_read_decodes_none() {
    let old = b"{\"n\":1}\n{\"n\":2}\n";
    let mut cache = RecordCache::default();
    let previous = seed(&mut cache, 1, old);
    assert_eq!((cache.decoded, cache.reused), (2, 0));
    let new = b"{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n";
    let next = candidate(1, new);
    let batch = cache.decode(&next, Some(&previous), new, committed(new));
    assert_eq!((cache.decoded, cache.reused), (3, 2));
    assert_matches_cold(&next, new, &batch);
    cache.retain(next.clone(), batch);
    let previous = parsed(next.clone(), new, None);
    let unchanged = cache.decode(&next, Some(&previous), new, committed(new));
    assert_eq!((cache.decoded, cache.reused), (3, 5));
    assert_matches_cold(&next, new, &unchanged);
    assert!(cache.entries.is_empty());
    assert_eq!(cache.weight, 0);
    cache.retain(next, unchanged);
    assert_accounting(&cache);
}

#[test]
fn unfinished_tail_is_not_decoded_until_its_newline_arrives() {
    let mut cache = RecordCache::default();
    let old = b"{\"n\":1}\n{\"n\":";
    let previous = seed(&mut cache, 1, old);
    assert_eq!(cache.decoded, 1);
    let more = b"{\"n\":1}\n{\"n\":2}";
    let next = candidate(1, more);
    let batch = cache.decode(&next, Some(&previous), more, committed(more));
    assert_eq!((cache.decoded, cache.reused), (1, 1));
    assert_eq!(batch.records.len(), 1);
    cache.retain(next.clone(), batch);
    let previous = parsed(next, more, None);
    let complete = b"{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n";
    let next = candidate(1, complete);
    let batch = cache.decode(&next, Some(&previous), complete, committed(complete));
    assert_eq!((cache.decoded, cache.reused), (3, 2));
    assert_matches_cold(&next, complete, &batch);
    assert_eq!(batch.records.last().unwrap().1, complete.len() as u64);
}

#[test]
fn complete_prefix_rewrite_after_four_kib_forces_full_decode() {
    for append in [false, true] {
        let mut old =
            format!("{{\"padding\":\"{}\"}}\n{{\"n\":2}}\n", "a".repeat(6000)).into_bytes();
        let mut cache = RecordCache::default();
        let previous = seed(&mut cache, 1, &old);
        old[5000] = b'b';
        assert_eq!(&old[..4096], previous.raw_index.head_bytes());
        if append {
            old.extend_from_slice(b"{\"n\":3}\n");
        }
        let next = candidate(1, &old);
        let batch = cache.decode(&next, Some(&previous), &old, committed(&old));
        // A rewrite invalidates the speculative suffix too: count its real
        // parse work before the complete cold second pass, not just accepted rows.
        assert_eq!(
            (cache.decoded, cache.reused),
            (4 + 2 * usize::from(append), 0)
        );
        assert_matches_cold(&next, &old, &batch);
    }
}

#[test]
fn truncating_committed_records_forces_full_decode() {
    let mut cache = RecordCache::default();
    let previous = seed(&mut cache, 1, b"{\"n\":1}\n{\"n\":2}\n");
    let bytes = b"{\"n\":1}\n";
    let next = candidate(1, bytes);
    let batch = cache.decode(&next, Some(&previous), bytes, committed(bytes));
    assert_eq!((cache.decoded, cache.reused), (3, 0));
    assert_matches_cold(&next, bytes, &batch);
}

#[test]
fn inode_and_candidate_identity_changes_never_reuse_rows() {
    for change in 0..5 {
        let mut cache = RecordCache::default();
        let previous = seed(&mut cache, 1, b"{\"n\":1}\n");
        let bytes = b"{\"n\":1}\n{\"n\":2}\n";
        let mut next = candidate(1, bytes);
        match change {
            0 => next.stamps[0].file_identity = "replacement-inode".into(),
            1 => next.root = PathBuf::from("different-authorized-root"),
            2 => next.data = next.root.join("different-data.jsonl"),
            3 => next.summary = Some(next.root.join("different-summary.json")),
            _ => next.source = "codex",
        }
        let batch = cache.decode(&next, Some(&previous), bytes, committed(bytes));
        assert_eq!(
            (cache.decoded, cache.reused),
            (3, 0),
            "identity change {change}"
        );
        assert_matches_cold(&next, bytes, &batch);
        assert_accounting(&cache);
    }
}

#[test]
fn missing_previous_mismatched_cached_generation_and_raw_error_force_cold_decode() {
    for reason in 0..3 {
        let mut cache = RecordCache::default();
        let mut previous = seed(&mut cache, 1, b"{\"n\":1}\n");
        if reason == 1 {
            previous.candidate.stamps[0].modified += 1;
        }
        if reason == 2 {
            previous.raw_error = Some("synthetic prior raw error".into());
        }
        let bytes = b"{\"n\":1}\n{\"n\":2}\n";
        let next = candidate(1, bytes);
        let batch = cache.decode(
            &next,
            (reason != 0).then_some(&previous),
            bytes,
            committed(bytes),
        );
        assert_eq!((cache.decoded, cache.reused), (3, 0));
        assert_matches_cold(&next, bytes, &batch);
    }
}

/// A complete line that is not a JSON object is skipped and counted;
/// the count survives cache reuse and the bytes
/// stay in the physical index.
#[test]
fn invalid_and_nonobject_rows_are_skipped_counted_and_retained() {
    let mut cache = RecordCache::default();
    let previous = seed(&mut cache, 1, b"{}\n");
    let bytes = b"{}\nnot-json\n[]\n{\"valid\":true}\n";
    let next = candidate(1, bytes);
    let batch = cache.decode(&next, Some(&previous), bytes, committed(bytes));
    assert_eq!((cache.decoded, cache.reused), (4, 1));
    assert!(batch.error.is_none(), "{:?}", batch.error);
    assert_eq!(batch.invalid, 2);
    assert_eq!(batch.records.len(), 2);
    assert_eq!(batch.records[1].1, bytes.len() as u64);
    assert_matches_cold(&next, bytes, &batch);
    let previous = parsed(next.clone(), bytes, None);
    cache.retain(next.clone(), batch);
    assert_eq!(cache.entries.len(), 1);
    let mut appended = bytes.to_vec();
    appended.extend_from_slice(b"42\n{\"more\":true}\n");
    let next = candidate(1, &appended);
    let batch = cache.decode(&next, Some(&previous), &appended, committed(&appended));
    assert_eq!((cache.decoded, cache.reused), (6, 3));
    assert!(batch.error.is_none());
    assert_eq!(batch.invalid, 3, "the cached prefix keeps its count");
    assert_eq!(batch.records.len(), 3);
    assert_matches_cold(&next, &appended, &batch);
    assert_eq!(
        invalid_lines_warning(batch.invalid).as_deref(),
        Some("跳过无效的JSONL 记录 ×3")
    );
    assert_eq!(invalid_lines_warning(0), None);
}

/// The observed real-root shape: a crash-time line of NUL bytes followed by
/// the tail of a record, between two valid records. Offsets, checkpoints and
/// the committed end are those of the whole file; only the row is missing.
#[test]
fn torn_nul_line_between_records_is_skipped_but_its_bytes_stay_in_the_index() {
    let first = b"{\"type\":\"user\",\"uuid\":\"u1\"}\n";
    let mut torn = vec![0u8; 4096];
    torn.extend_from_slice("\"…tail\"}\n".as_bytes());
    let last = b"{\"type\":\"assistant\",\"uuid\":\"a1\"}\n";
    let mut bytes = first.to_vec();
    bytes.extend_from_slice(&torn);
    bytes.extend_from_slice(last);
    let source = candidate(7, &bytes);
    let mut cache = RecordCache::default();
    let (batch, index) = cache
        .decode_input(&source, None, |decoder, probe| {
            scan_records(bytes.as_slice(), decoder, probe)
        })
        .unwrap();
    assert!(batch.error.is_none(), "{:?}", batch.error);
    assert_eq!(batch.invalid, 1);
    assert_eq!(batch.records.len(), 2);
    assert_eq!(batch.records[0].1, first.len() as u64);
    assert_eq!(
        batch.records[1].1,
        bytes.len() as u64,
        "the record after the torn line keeps its physical offset"
    );
    assert_eq!(index.committed(), bytes.len() as u64);
    let torn_end = (first.len() + torn.len()) as u64;
    assert!(
        index.is_checkpoint(torn_end),
        "the torn line's LF is a checkpoint"
    );
    assert!(index.is_checkpoint(first.len() as u64));
    assert!(index.is_checkpoint(bytes.len() as u64));
    assert!(!index.is_checkpoint(torn_end - 1));
    // Invalid complete JSON stays skippable even above the former record quota.
    let mut oversized = first.to_vec();
    oversized.extend_from_slice(&vec![0u8; LINE_LIMIT + 1]);
    oversized.push(b'\n');
    oversized.extend_from_slice(last);
    let source = candidate(8, &oversized);
    let batch = RecordCache::default().decode(&source, None, &oversized, committed(&oversized));
    assert!(batch.error.is_none());
    assert_eq!(batch.invalid, 1);
    assert_eq!(batch.records.len(), 2);
}

#[test]
fn records_above_former_line_limit_survive_cache_reuse() {
    let exact = format!(
        "{{\"p\":\"{}\"}}\n",
        "x".repeat(LINE_LIMIT - b"{\"p\":\"\"}\n".len())
    )
    .into_bytes();
    assert_eq!(exact.len(), LINE_LIMIT);
    // A 64 MiB record weighs more than the default 64 MiB AST budget; this
    // test checks append reuse, so give the cache enough retention capacity.
    let mut cache = RecordCache::with_budget(1 << 30, 8);
    let previous = seed(&mut cache, 1, &exact);
    let mut next_bytes = exact.clone();
    let mut oversized = exact;
    oversized.insert(7, b'x');
    next_bytes.extend(oversized);
    let next = candidate(1, &next_bytes);
    let batch = cache.decode(&next, Some(&previous), &next_bytes, committed(&next_bytes));
    assert_eq!((cache.decoded, cache.reused), (2, 1));
    assert_eq!(batch.records.len(), 2);
    assert!(batch.error.is_none());
    assert_matches_cold(&next, &next_bytes, &batch);
    cache.retain(next, batch);
    assert!(cache.weight > 0);
}

#[test]
fn more_than_a_million_rows_survive_cache_reuse() {
    let mut bytes = b"{}\n".repeat(ROW_LIMIT);
    // A million rows weigh ~100 MB; keep them cached to check append reuse.
    let mut cache = RecordCache::with_budget(1 << 30, 8);
    let previous = seed(&mut cache, 1, &bytes);
    assert_eq!(cache.decoded, ROW_LIMIT);
    bytes.extend_from_slice(b" \n\t\n{}\n");
    let next = candidate(1, &bytes);
    let batch = cache.decode(&next, Some(&previous), &bytes, committed(&bytes));
    assert_eq!((cache.decoded, cache.reused), (ROW_LIMIT + 1, ROW_LIMIT));
    assert_eq!(batch.records.len(), ROW_LIMIT + 1);
    assert!(batch.error.is_none());
    assert_matches_cold(&next, &bytes, &batch);
    cache.retain(next, batch);
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.weight > 0);
}

#[test]
fn sixteen_entry_lru_eviction_changes_cost_not_results() {
    #[allow(non_snake_case)]
    let MAX_ENTRIES = max_entries();
    let bytes = b"{\"n\":1}\n";
    let mut cache = RecordCache::default();
    let previous = (0..MAX_ENTRIES)
        .map(|id| seed(&mut cache, id, bytes))
        .collect::<Vec<_>>();
    let batch = cache.decode(
        &previous[0].candidate,
        Some(&previous[0]),
        bytes,
        committed(bytes),
    );
    cache.retain(previous[0].candidate.clone(), batch);
    seed(&mut cache, MAX_ENTRIES, bytes);
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    assert!(
        cache
            .entries
            .contains_key(&uid_for("claude", &previous[0].candidate.path))
    );
    assert!(
        !cache
            .entries
            .contains_key(&uid_for("claude", &previous[1].candidate.path))
    );
    let before = cache.decoded;
    let batch = cache.decode(
        &previous[1].candidate,
        Some(&previous[1]),
        bytes,
        committed(bytes),
    );
    assert_eq!(cache.decoded, before + 1);
    assert_matches_cold(&previous[1].candidate, bytes, &batch);
    cache.retain(previous[1].candidate.clone(), batch);
    assert_accounting(&cache);
}

#[test]
fn weight_boundary_evicts_old_entries_and_oversized_batches_are_disposable() {
    // Test exact logical-weight admission independently of allocator capacities.
    #[allow(non_snake_case)]
    let MAX_WEIGHT = max_weight();
    let mut cache = RecordCache::default();
    let make = |weight| Batch {
        records: vec![(json!({}), 3)],
        sidecars: BTreeMap::new(),
        error: None,
        invalid: 0,
        committed: 3,
        weight,
    };
    cache.retain(candidate(0, b"{}\n"), make(MAX_WEIGHT));
    assert_eq!(cache.weight, MAX_WEIGHT);
    cache.retain(candidate(1, b"{}\n"), make(1));
    assert_eq!(cache.weight, 1);
    assert_eq!(cache.entries.len(), 1);
    cache.retain(candidate(2, b"{}\n"), make(MAX_WEIGHT + 1));
    assert_eq!(cache.weight, 1);
    assert_eq!(cache.entries.len(), 1);
    assert_accounting(&cache);
}

#[test]
fn many_small_json_nodes_are_readable_without_cache_admission() {
    let node_count = 3_200_001;
    let mut bytes = serde_json::to_vec(&json!({"items": vec![Value::Null; node_count]})).unwrap();
    bytes.push(b'\n');
    let next = candidate(1, &bytes);
    let mut cache = RecordCache::default();
    let batch = cache.decode(&next, None, &bytes, committed(&bytes));
    assert!(batch.error.is_none(), "{:?}", batch.error);
    assert_eq!(batch.invalid, 0);
    assert_eq!(
        batch.records[0].0["items"].as_array().unwrap().len(),
        node_count
    );
    cache.retain(next, batch);
    assert!(
        cache.entries.is_empty(),
        "large AST is readable but not cached"
    );
    assert_accounting(&cache);
}

#[test]
fn summary_metadata_change_reuses_records_but_not_a_different_data_inode() {
    let bytes = b"{}\n";
    let mut old = candidate(1, bytes);
    old.source = "grok";
    old.summary = Some(old.root.join("metadata.json"));
    old.stamps.push(FileStamp {
        size: 20,
        modified: 1,
        identity: "summary-inode".into(),
        file_identity: "summary-inode".into(),
    });
    let mut cache = RecordCache::default();
    let batch = cache.decode(&old, None, bytes, committed(bytes));
    cache.retain(old.clone(), batch);
    let previous = parsed(old.clone(), bytes, None);
    let mut next = old;
    next.stamps[1].modified += 1;
    next.stamps[1].size += 1;
    let batch = cache.decode(&next, Some(&previous), bytes, committed(bytes));
    assert_eq!((cache.decoded, cache.reused), (1, 1));
    assert_matches_cold(&next, bytes, &batch);
    cache.retain(next.clone(), batch);
    let previous = parsed(next.clone(), bytes, None);
    next.stamps[0].file_identity = "replacement-chat-inode".into();
    let batch = cache.decode(&next, Some(&previous), bytes, committed(bytes));
    assert_eq!((cache.decoded, cache.reused), (2, 1));
    assert_matches_cold(&next, bytes, &batch);
}

#[test]
fn streaming_chunks_preserve_blank_crlf_utf8_and_complete_byte_offsets() {
    use std::io::Read;
    let bytes = "\n \r\n{\"z\":\"中文🔒\",\"a\":2}\r\n\n{\"pending\":".as_bytes();
    for chunk in [1, 2, 7, 65536] {
        struct Chunked<'a>(&'a [u8], usize);
        impl Read for Chunked<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.len().min(self.1).min(buffer.len());
                buffer[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let mut decoder = Decoder::cold();
        let index = scan_records(Chunked(bytes, chunk), &mut decoder, None).unwrap();
        let batch = decoder.finish(index.committed() as usize);
        assert!(batch.error.is_none());
        assert_eq!(batch.records.len(), 1);
        assert_eq!(batch.records[0].1, index.committed() - 1);
        assert_eq!(
            serde_json::to_string(&batch.records[0].0).unwrap(),
            "{\"z\":\"中文🔒\",\"a\":2}"
        );
        assert_eq!(index.committed() as usize, committed(bytes));
    }
}

#[test]
fn oversized_uncommitted_line_waits_for_lf() {
    let mut decoder = Decoder::cold();
    decoder.feed(b"{}\n\"");
    let feeds = LINE_LIMIT / 65536 + 8;
    for _ in 0..feeds {
        decoder.feed(&[b'x'; 65536]);
    }
    assert!(decoder.error.is_none());
    assert!(decoder.line.len() > LINE_LIMIT);
    assert_eq!(decoder.records.len(), 1);
    decoder.feed(b"\"\n");
    let batch = decoder.finish(feeds * 65536 + 6);
    assert!(batch.error.is_none());
    assert_eq!(batch.invalid, 1);
}

#[test]
fn prefix_probe_failure_reopens_once_and_never_returns_stale_cached_rows() {
    let mut cache = RecordCache::default();
    let old = b"{\"z\":1}\n";
    let previous = seed(&mut cache, 1, old);
    let new = b"{\"z\":2}\n{\"a\":3}\n";
    let next = candidate(1, new);
    let mut probes = Vec::new();
    let (batch, index) = cache
        .decode_input(&next, Some(&previous), |decoder, probe| {
            probes.push(probe);
            scan_records(new.as_slice(), decoder, probe)
        })
        .unwrap();
    assert_eq!(probes, [Some(old.len() as u64), None]);
    assert_eq!(index.length(), new.len() as u64);
    assert_eq!(batch.records[0].0["z"], 2);
    assert_eq!(cache.reused, 0);
    assert_eq!(cache.decoded, 4); // old seed + speculative suffix + full 2-row pass
    assert_matches_cold(&next, new, &batch);
}

#[test]
fn failed_second_pass_does_not_restore_or_publish_the_stale_cache_entry() {
    let mut cache = RecordCache::default();
    let previous = seed(&mut cache, 1, b"{\"n\":1}\n");
    let changed = b"{\"n\":2}\n";
    let next = candidate(1, changed);
    let mut calls = 0;
    let result = cache.decode_input(&next, Some(&previous), |decoder, probe| {
        calls += 1;
        if calls == 2 {
            return Err(SessionError::new(503, "synthetic source changed"));
        }
        scan_records(changed.as_slice(), decoder, probe)
    });
    assert_eq!(result.err().unwrap().status, 503);
    assert_eq!(calls, 2);
    assert!(cache.entries.is_empty());
    assert_eq!(cache.weight, 0);
    let recovered = cache.decode(&next, None, changed, changed.len());
    assert_eq!(recovered.records[0].0["n"], 2);
}

#[test]
fn long_object_key_is_valid_native_json() {
    let key = "k".repeat(16 * 1024 + 1);
    let raw = serde_json::to_vec(&json!({key.clone(): "value"})).unwrap();
    assert_eq!(decode_record(&raw).unwrap()[&key], "value");
}
