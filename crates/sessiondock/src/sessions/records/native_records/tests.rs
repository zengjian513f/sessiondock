use super::*;
const MAX_CHECKPOINTS: usize = 2_000_000;
const LINE_LIMIT: usize = 64 * 1024 * 1024;
use crate::sessions::FileStamp;

#[test]
fn complete_but_uncommitted_tool_string_never_opens_a_replay_source() {
    let envelope = serde_json::json!({"wall_time_seconds":1,"exit_code":0,
        "output":{"content":[{"type":"image","mime_type":"image/png",
            "data":"A".repeat(SPAN_THRESHOLD + 4)}]}})
    .to_string();
    let bytes = serde_json::to_vec(&serde_json::json!({"type":"response_item",
        "payload":{"type":"function_call_output","output":envelope}}))
    .unwrap();
    let mut source = candidate(&bytes);
    source.source = "codex";
    let mut decoder = Decoder::cold();
    // The synthetic candidate has no file. Replaying before LF would try to open it;
    // instead the uncommitted complete JSON is ignored without any source open.
    let index = scan_native_records(bytes.as_slice(), &mut decoder, None, &source).unwrap();
    let batch = decoder.finish(index.committed() as usize);
    assert_eq!(index.committed(), 0);
    assert!(batch.records.is_empty() && batch.sidecars.is_empty());
    assert!(batch.error.is_none());
}

fn candidate(bytes: &[u8]) -> Candidate {
    // Absolute synthetic metadata prevents path validation from masking an
    // accidental authorization of a non-image span. This scanner opens no file.
    let root = std::env::temp_dir().join("synthetic-pull-root");
    let path = root.join("records.jsonl");
    Candidate {
        source: "claude",
        root,
        data: path.clone(),
        path,
        summary: None,
        stamps: vec![FileStamp {
            size: bytes.len() as u64,
            modified: bytes.len() as u128,
            identity: "synthetic-stamp".into(),
            file_identity: "synthetic-file-identity".into(),
        }],
    }
}

struct Chunked<'a> {
    bytes: &'a [u8],
    maximum: usize,
    interrupt_once: bool,
}
impl Read for Chunked<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.interrupt_once {
            self.interrupt_once = false;
            return Err(io::ErrorKind::Interrupted.into());
        }
        let count = buffer.len().min(self.maximum).min(self.bytes.len());
        buffer[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}
fn pull(bytes: &[u8], chunk: usize) -> (super::super::Batch, RawIndex) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("records.jsonl");
    std::fs::write(&path, bytes).unwrap();
    let source = Candidate {
        source: "claude",
        root,
        data: path.clone(),
        path: path.clone(),
        summary: None,
        stamps: vec![crate::sessions::stamp(&path).unwrap()],
    };
    let mut decoder = Decoder::cold();
    let index = scan_native_records(
        Chunked {
            bytes,
            maximum: chunk,
            interrupt_once: false,
        },
        &mut decoder,
        None,
        &source,
    )
    .unwrap();
    let batch = decoder.finish(index.committed() as usize);
    (batch, index)
}

#[test]
fn first_underlying_io_failure_cannot_be_swallowed_by_scanner_or_drain() {
    struct FailOnce<'a> {
        bytes: &'a [u8],
        position: usize,
        fail_at: usize,
        failed: bool,
        reads_after_failure: usize,
    }
    impl Read for FailOnce<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.failed {
                self.reads_after_failure += 1;
            }
            if !self.failed && self.position == self.fail_at {
                self.failed = true;
                return Err(io::Error::other("SECRET native path and payload"));
            }
            let end = if self.failed {
                self.bytes.len()
            } else {
                self.fail_at
            };
            let count = output.len().min(end - self.position);
            output[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
            self.position += count;
            Ok(count)
        }
    }
    let bytes = format!("{{\"text\":\"{}\"}}\n", "x".repeat(SMALL * 2)).into_bytes();
    // Includes failure during the scanner's own pull, after the small prefix.
    for fail_at in [0, 1, SMALL - 1, SMALL, SMALL + 17] {
        let mut reader = FailOnce {
            bytes: &bytes,
            position: 0,
            fail_at,
            failed: false,
            reads_after_failure: 0,
        };
        let mut decoder = Decoder::cold();
        let error = scan_native_records(&mut reader, &mut decoder, None, &candidate(&bytes))
            .err()
            .unwrap();
        assert_eq!(error.status, 503, "failure at {fail_at}");
        assert!(!error.message.contains("SECRET"));
        assert_eq!(
            reader.reads_after_failure, 0,
            "first error is sticky before underlying retry"
        );
        assert!(decoder.records.is_empty());
        assert!(decoder.sidecars.is_empty());
    }
}

#[test]
fn all_lf_checkpoints_are_retained() {
    for lines in [MAX_CHECKPOINTS, MAX_CHECKPOINTS + 1] {
        let bytes = vec![b'\n'; lines];
        let mut decoder = Decoder::cold();
        let result = scan_native_records(bytes.as_slice(), &mut decoder, None, &candidate(&bytes));
        assert!(result.is_ok());
        assert!(decoder.records.is_empty());
        assert_eq!(decoder.invalid, 0);
        if let Ok(index) = result {
            assert_eq!(index.committed(), lines as u64);
        }
    }
}

#[test]
fn oversized_partial_tail_is_not_published_until_physical_lf() {
    let mut bytes = b"{\"ok\":1}\n".to_vec();
    let committed = bytes.len();
    bytes.extend_from_slice(b"{\"text\":\"");
    bytes.extend(std::iter::repeat_n(b'x', LINE_LIMIT + 1));
    for suffix in [b"".as_slice(), b"\"}", b"\"}\n"] {
        let mut input = bytes.clone();
        input.extend_from_slice(suffix);
        let (batch, index) = pull(&input, SMALL);
        let complete = suffix.ends_with(b"\n");
        assert_eq!(batch.records.len(), if complete { 2 } else { 1 });
        assert!(batch.sidecars.is_empty());
        assert_eq!(batch.invalid, 0);
        assert!(batch.error.is_none(), "{:?}", batch.error);
        assert_eq!(
            index.committed() as usize,
            if complete { input.len() } else { committed }
        );
    }
}

/// Ordinary strings above the span threshold are read back from the stamped
/// file (verified length/SHA-1), never turned into image authority; over the
/// without a file to read from, the record fails closed.
#[test]
fn giant_ordinary_text_is_materialized_from_the_file_but_never_becomes_an_image() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("records.jsonl");
    let data = format!("{}中文😀\"quoted\"", "A".repeat(SPAN_THRESHOLD + 1));
    let value = serde_json::json!({"type":"user","message":{"content":[{"type":"text","text":data}]},
        "_native_span":{"start":0,"end":200,"source":"image","authorized":true}});
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    std::fs::write(&path, &bytes).unwrap();
    let source = Candidate {
        source: "claude",
        root: root.clone(),
        data: path.clone(),
        path: path.clone(),
        summary: None,
        stamps: vec![crate::sessions::stamp(&path).unwrap()],
    };
    let mut decoder = Decoder::cold();
    let index = scan_native_records(bytes.as_slice(), &mut decoder, None, &source).unwrap();
    assert_eq!(index.committed(), bytes.len() as u64);
    let batch = decoder.finish(index.committed() as usize);
    assert!(batch.error.is_none(), "{:?}", batch.error);
    assert_eq!(batch.records.len(), 1);
    assert!(batch.sidecars.is_empty());
    assert_eq!(batch.records[0].0, value);
    assert_eq!(
        serde_json::to_vec(&batch.records[0].0).unwrap(),
        bytes[..bytes.len() - 1]
    );
    // The same physical bytes under a rewritten file are not trusted.
    let mut changed = bytes.clone();
    let at = changed.iter().position(|byte| *byte == b'A').unwrap();
    changed[at] = b'B';
    std::fs::write(&path, &changed).unwrap();
    let mut decoder = Decoder::cold();
    let error = scan_native_records(bytes.as_slice(), &mut decoder, None, &source)
        .err()
        .expect("rewritten source is not trusted");
    assert!(matches!(error.status, 409 | 503), "{}", error.message);
}

#[test]
fn giant_ordinary_strings_or_public_marker_fields_do_not_authorize_spans() {
    let data = "A".repeat(LINE_LIMIT + 1);
    for value in [
        serde_json::json!({"text":data}),
        serde_json::json!({"type":"user","message":{"content":[{"type":"text","text":data}]}}),
        serde_json::json!({"type":"user","message":{"content":[{"type":"text","text":data}]},
            "_native_span":{"start":0,"end":200,"source":"image","authorized":true}}),
        serde_json::json!({"metadata":{"type":"image","source":{"type":"base64","media_type":"image/png","data":data}}}),
    ] {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        let (batch, index) = pull(&bytes, SMALL);
        assert_eq!(index.committed(), bytes.len() as u64);
        assert!(batch.error.is_none(), "{:?}", batch.error);
        assert_eq!(batch.records.len(), 1);
        assert_eq!(batch.records[0].0, value);
        assert!(batch.sidecars.is_empty());
    }
}

#[test]
fn actual_structured_image_span_is_retained_privately_without_container_decode() {
    let data = "A".repeat(SPAN_THRESHOLD + 4);
    let value = serde_json::json!({"type":"user","message":{"content":[
        {"type":"text","text":"text survives"},
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":data}}
    ]}});
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    let (batch, index) = pull(&bytes, SMALL);
    assert!(batch.error.is_none(), "{:?}", batch.error);
    assert_eq!(batch.records.len(), 1);
    assert_eq!(batch.records[0].1, bytes.len() as u64);
    let sidecars = &batch.sidecars[&(bytes.len() as u64)];
    assert_eq!(sidecars.len(), 1);
    let span = sidecars[0].image.as_ref().unwrap().native_span().unwrap();
    assert_eq!(span.decoded_len, data.len() as u64);
    assert_eq!(
        &bytes[span.start as usize..span.end as usize],
        data.as_bytes()
    );
    assert_eq!(index.committed(), bytes.len() as u64);
    let public_record = serde_json::to_string(&batch.records[0].0).unwrap();
    assert!(public_record.contains("text survives"));
    assert!(public_record.len() < 4096);
    assert!(!public_record.contains(&data));
}

#[test]
fn tiny_chunks_preserve_unicode_crlf_blank_lines_order_and_physical_offsets() {
    let bytes = "\n \r\n{\"z\":\"中文😀\",\"a\":2}\r\n\n{\"b\":3}\n{\"partial\":".as_bytes();
    let mut old_decoder = Decoder::cold();
    let old_index = scan_records(bytes, &mut old_decoder, None).unwrap();
    let old = old_decoder.finish(old_index.committed() as usize);
    for chunk in [1, 2, 3, 7, SMALL] {
        let (batch, index) = pull(bytes, chunk);
        assert_eq!(batch.error, old.error);
        assert_eq!(batch.records, old.records);
        assert_eq!(
            serde_json::to_vec(&batch.records).unwrap(),
            serde_json::to_vec(&old.records).unwrap()
        );
        assert_eq!(index.length(), old_index.length());
        assert_eq!(index.committed(), old_index.committed());
        assert_eq!(index.digest(), old_index.digest());
        assert_eq!(index.committed_digest(), old_index.committed_digest());
        for end in 0..=bytes.len() as u64 {
            assert_eq!(index.prefix_hash(end), old_index.prefix_hash(end));
        }
    }
}

#[test]
fn pull_probe_hashes_the_skipped_prefix_and_interrupted_reads_remain_retryable() {
    let bytes = b"{\"old\":1}\n{\"new\":2}\n";
    let skip = b"{\"old\":1}\n".len();
    let mut decoder = Decoder::cold();
    decoder.skip = skip as u64;
    let index = scan_native_records(
        Chunked {
            bytes,
            maximum: 3,
            interrupt_once: true,
        },
        &mut decoder,
        Some(skip as u64),
        &candidate(bytes),
    )
    .unwrap();
    let old = RawIndex::scan(&bytes[..skip]).unwrap();
    assert_eq!(index.probe_digest(), Some(old.committed_digest()));
    assert_eq!(decoder.records.len(), 1);
    assert_eq!(decoder.records[0].0["new"], 2);
    assert_eq!(decoder.records[0].1, bytes.len() as u64);
}

#[test]
fn cold_read_retries_first_interrupted_io_without_skipping_or_double_counting_bytes() {
    let bytes = b"{\"first\":1}\n{\"second\":2}\n";
    let mut decoder = Decoder::cold();
    assert_eq!(decoder.skip, 0);
    let index = scan_native_records(
        Chunked {
            bytes,
            maximum: 3,
            interrupt_once: true,
        },
        &mut decoder,
        None,
        &candidate(bytes),
    )
    .unwrap();
    assert_eq!(decoder.records.len(), 2);
    assert_eq!(decoder.records[0].0["first"], 1);
    assert_eq!(decoder.records[0].1, b"{\"first\":1}\n".len() as u64);
    assert_eq!(decoder.records[1].0["second"], 2);
    assert_eq!(decoder.records[1].1, bytes.len() as u64);
    let expected = RawIndex::scan(bytes.as_slice()).unwrap();
    assert_eq!(index.length(), bytes.len() as u64);
    assert_eq!(index.digest(), expected.digest());
    assert_eq!(index.committed_digest(), expected.committed_digest());
    assert!(decoder.finish(index.committed() as usize).error.is_none());
}

#[test]
fn image_span_and_large_ordinary_body_are_both_preserved() {
    let image = "A".repeat(SPAN_THRESHOLD + 4);
    // Ordinary text stays inline (each block below the span threshold), while
    // the image forces the authorized-span branch. Both below and above the
    // old record quota, all ordinary text and the image marker survive.
    let block = "x".repeat(SPAN_THRESHOLD - 1024);
    let blocks = LINE_LIMIT / block.len();
    for count in [blocks - 1, blocks + 1] {
        let mut content = (0..count)
            .map(|_| serde_json::json!({"type":"text","text":block}))
            .collect::<Vec<_>>();
        content.push(serde_json::json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":image}}));
        let value = serde_json::json!({"type":"user","message":{"content":content}});
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        let (batch, index) = pull(&bytes, SMALL);
        assert_eq!(index.committed(), bytes.len() as u64);
        assert!(batch.error.is_none(), "{:?}", batch.error);
        assert_eq!(batch.records.len(), 1);
        assert_eq!(batch.sidecars[&(bytes.len() as u64)].len(), 1);
        let content = batch.records[0].0["message"]["content"].as_array().unwrap();
        assert_eq!(content.len(), count + 1);
        assert!(content[..count].iter().all(|part| part["text"] == block));
        assert_eq!(content[count]["type"], "image");
    }
}

/// On the pull path too, a complete line that is not a JSON object
/// is skipped and counted whatever its length (small-record and streaming
/// scanner alike), the following records keep their physical offsets, and a
/// an unsupported media spelling does not turn valid JSON into a hard failure.
#[test]
fn invalid_lines_are_skipped_and_unsupported_media_does_not_fail_the_pull_path() {
    let first = b"{\"ok\":1}\n";
    let mut bytes = first.to_vec();
    bytes.extend_from_slice(b"not json\n");
    bytes.extend_from_slice(b"[1,2]\n");
    bytes.extend(std::iter::repeat_n(b'\x00', SMALL + 10));
    bytes.extend_from_slice("\"…tail\"}\n".as_bytes());
    bytes.extend_from_slice(b"{\"text\":\"");
    bytes.extend(std::iter::repeat_n(b'x', SMALL * 2));
    bytes.push(b'\n');
    let last = b"{\"ok\":2}\n";
    bytes.extend_from_slice(last);
    let (batch, index) = pull(&bytes, SMALL);
    assert!(batch.error.is_none(), "{:?}", batch.error);
    assert_eq!(batch.invalid, 4);
    assert_eq!(batch.records.len(), 2);
    assert_eq!(batch.records[0].1, first.len() as u64);
    assert_eq!(batch.records[1].1, bytes.len() as u64);
    assert!(batch.sidecars.is_empty());
    assert_eq!(index.committed(), bytes.len() as u64);
    assert!(index.is_checkpoint((bytes.len() - last.len()) as u64));

    let data = "A".repeat(SPAN_THRESHOLD + 4);
    let value = serde_json::json!({"type":"user","message":{"content":[
        {"type":"image","source":{"type":"base64","media_type":"image/svg+xml","data":data}}
    ]}});
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    let (batch, index) = pull(&bytes, SMALL);
    assert_eq!(index.committed(), bytes.len() as u64);
    assert!(batch.error.is_none(), "{:?}", batch.error);
    assert_eq!(batch.invalid, 0);
    assert_eq!(batch.records.len(), 1);
    assert!(batch.sidecars.is_empty());
    assert_eq!(
        batch.records[0].0["message"]["content"][0]["source"]["data"]
            .as_str()
            .unwrap()
            .len(),
        data.len()
    );
}

/// A multi-part Codex output (`[header, chunk, chunk, …]`)
/// is not one envelope candidate. Every giant chunk part is decoded in place
/// through streaming replay (a span candidate, never read back as text and
/// parsed a second time), a giant ordinary part is still read back
/// verbatim, and an image part next to the chunks becomes a sidecar; the
/// provider then shows every chunk's output.
#[test]
fn multi_part_tool_output_decodes_giant_chunks_in_place() {
    use super::super::native_images::PathPart;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("records.jsonl");
    // Decoded output ≤ 2 MiB stays inline inside the decoded layer while the
    // escaped newlines push the chunk part itself over the span threshold.
    let first = format!("{}{}", "x".repeat(SPAN_THRESHOLD - 1024), "\n".repeat(600));
    let second = format!("{}{}", "y".repeat(SPAN_THRESHOLD - 1024), "\n".repeat(600));
    let chunk = |id: &str, output: &str, exit: i64| {
        serde_json::json!({"chunk_id":id,"wall_time_seconds":0.5,"exit_code":exit,"output":output})
            .to_string()
    };
    assert!(chunk("g", &first, 0).len() > SPAN_THRESHOLD);
    let plain = "p".repeat(SPAN_THRESHOLD + 1);
    let image = "A".repeat(SPAN_THRESHOLD + 4);
    let value = serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call_output",
        "call_id":"call","output":[
            {"type":"input_text","text":"Script completed\nWall time 0.5 seconds\nOutput:\n"},
            {"type":"input_text","text":chunk("s", "small\n", 0)},
            {"type":"input_text","text":chunk("g", &first, 0)},
            {"type":"input_text","text":plain},
            {"type":"input_text","text":chunk("h", &second, 3)},
            {"type":"input_image","image_url":format!("data:image/png;base64,{image}")},
            {"type":"input_text","text":""}]}});
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    std::fs::write(&path, &bytes).unwrap();
    let source = Candidate {
        source: "codex",
        root: root.clone(),
        data: path.clone(),
        path: path.clone(),
        summary: None,
        stamps: vec![crate::sessions::stamp(&path).unwrap()],
    };
    let mut decoder = Decoder::cold();
    let index = scan_native_records(bytes.as_slice(), &mut decoder, None, &source).unwrap();
    assert_eq!(index.committed(), bytes.len() as u64);
    let batch = decoder.finish(index.committed() as usize);
    assert!(batch.error.is_none(), "{:?}", batch.error);
    assert_eq!(batch.records.len(), 1);
    let parts = batch.records[0].0["payload"]["output"].as_array().unwrap();
    assert_eq!(parts.len(), 7);
    assert_eq!(parts[1]["text"], chunk("s", "small\n", 0));
    // Giant chunks are structural envelopes now, not text parts.
    assert_eq!(parts[2]["chunk_id"], "g");
    assert_eq!(parts[2]["output"], first);
    assert_eq!(parts[4]["chunk_id"], "h");
    assert_eq!(parts[4]["output"], second);
    assert_eq!(parts[3]["text"], plain);
    assert!(parts[5].get("image_url").is_none());
    assert_eq!(parts[6]["text"], "");
    let sidecars = &batch.sidecars[&(bytes.len() as u64)];
    assert_eq!(sidecars.len(), 1);
    assert_eq!(
        sidecars[0].path,
        [
            PathPart::Key("payload".into()),
            PathPart::Key("output".into()),
            PathPart::Index(5)
        ]
    );
    let span = sidecars[0].image.as_ref().unwrap().native_span().unwrap();
    assert_eq!(
        span.decoded_len,
        image.len() as u64 + "data:image/png;base64,".len() as u64
    );
    assert!(
        span.plan.is_none(),
        "a direct image part has no decode plan"
    );
    let (_, events, error) = crate::sessions::providers::parse_with_media(
        "codex",
        &path,
        &batch.records,
        None,
        "fixture",
        &batch.sidecars,
    );
    assert_eq!(error, None);
    let result = events
        .iter()
        .find(|event| event.message["role"] == "tool_result")
        .unwrap();
    assert_eq!(
        result.message["text"].as_str().unwrap(),
        format!("small\n{first}{second}")
    );
    assert_eq!(result.message["exit_code"], 3);
    assert_eq!(result.message["duration_s"], 1.5);
    assert_eq!(result.media.len(), 1);
    assert!(!result.message.to_string().contains(&plain));
}
