use super::*;
use sha1::{Digest, Sha1};
use std::io::Cursor;

fn hash(bytes: &[u8]) -> [u8; 20] {
    Sha1::digest(bytes).into()
}
fn range(start: u64, physical: &[u8], decoded: &[u8]) -> StringRange {
    StringRange {
        start,
        end: start + physical.len() as u64,
        decoded_len: decoded.len() as u64,
        decoded_sha1: hash(decoded),
    }
}
fn escaped(bytes: &[u8]) -> Vec<u8> {
    let quoted = serde_json::to_vec(std::str::from_utf8(bytes).unwrap()).unwrap();
    quoted[1..quoted.len() - 1].to_vec()
}
fn nested(depth: usize, tail: usize) -> (Vec<u8>, DecodePlan, Vec<u8>) {
    let expected = "a/é😀".as_bytes().to_vec();
    let mut raw = br"a\/\u00e9\ud83d\ude00".to_vec();
    let mut ranges = vec![range(0, &raw, &expected)];
    for _ in 1..depth {
        let mut text = "前缀:".as_bytes().to_vec();
        text.push(b'"');
        let start = text.len() as u64;
        text.extend_from_slice(&raw);
        ranges[0].start = start;
        ranges[0].end = start + raw.len() as u64;
        text.push(b'"');
        text.extend(std::iter::repeat_n(b'T', tail));
        raw = escaped(&text);
        ranges.insert(0, range(0, &raw, &text));
    }
    (raw, DecodePlan::new(ranges).unwrap(), expected)
}
struct Chunked<'a> {
    bytes: &'a [u8],
    chunk: usize,
    interrupted: bool,
    maximum: usize,
}
impl Read for Chunked<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.maximum = self.maximum.max(output.len());
        if self.interrupted {
            self.interrupted = false;
            return Err(io::ErrorKind::Interrupted.into());
        }
        let count = output.len().min(self.chunk).min(self.bytes.len());
        output[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}
#[test]
fn one_through_twelve_layers_decode_logical_ranges_and_verify_every_parent() {
    for depth in 1..=12 {
        let (raw, plan, expected) = nested(depth, 11);
        for chunk in [1, 3, CHUNK] {
            let source = Chunked {
                bytes: &raw,
                chunk,
                interrupted: true,
                maximum: 0,
            };
            let mut reader = ReplayReader::new(source, &plan).unwrap();
            let mut output = Vec::new();
            let mut buffer = [0; 2];
            loop {
                let count = reader.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                output.extend_from_slice(&buffer[..count]);
            }
            assert_eq!(output, expected);
            let source = reader.finish().unwrap();
            assert!(source.bytes.is_empty());
            assert!(source.maximum <= CHUNK);
        }
    }
}

#[test]
fn plans_accept_large_ranges_and_layers_but_reject_invalid_bounds() {
    assert!(DecodePlan::new(Vec::new()).is_err());
    let (_, plan, _) = nested(9, 1);
    let mut ten = plan.ranges().to_vec();
    ten.push(range(0, b"a", b"a"));
    assert!(DecodePlan::new(ten).is_ok());
    for invalid_range in [
        StringRange {
            start: 2,
            end: 1,
            decoded_len: 0,
            decoded_sha1: hash(b""),
        },
        StringRange {
            start: 0,
            end: 1,
            decoded_len: 2,
            decoded_sha1: hash(b"ab"),
        },
    ] {
        assert!(DecodePlan::new(vec![invalid_range]).is_err());
    }
    assert!(DecodePlan::new(vec![range(0, b"abc", b"abc"), range(3, b"x", b"x")]).is_err());
    let plan = DecodePlan::new(vec![range(u64::MAX - 1, b"x", b"x")]).unwrap();
    assert!(plan.with_outer_offset(1).is_err());
    let edge = StringRange {
        start: u64::MAX - 1,
        end: u64::MAX,
        decoded_len: 0,
        decoded_sha1: hash(b""),
    };
    assert!(DecodePlan::new(vec![edge]).is_ok());
}

#[test]
fn outer_offset_does_not_shift_children_or_cause_source_seeking() {
    let (raw, original, expected) = nested(3, 9);
    let shifted = original.with_outer_offset(3456).unwrap();
    assert_eq!(shifted.first().start, original.first().start + 3456);
    assert_eq!(shifted.first().end, original.first().end + 3456);
    assert_eq!(&shifted.ranges()[1..], &original.ranges()[1..]);
    assert_eq!(shifted.last(), original.last());
    assert_eq!(original.clone(), original);
    assert_eq!(
        shifted.resident_len(),
        std::mem::size_of::<DecodePlan>() + 3 * std::mem::size_of::<StringRange>()
    );
    let mut replay = ReplayReader::new(Cursor::new(&raw), &shifted).unwrap();
    let mut output = Vec::new();
    replay.read_to_end(&mut output).unwrap();
    assert_eq!(output, expected);
    assert_eq!(replay.finish().unwrap().position(), raw.len() as u64);
}

#[test]
fn unread_parent_tail_mutation_is_rejected_after_successful_child_eof() {
    for depth in [2, 3, 9] {
        let (mut raw, plan, expected) = nested(depth, CHUNK * 2);
        *raw.last_mut().unwrap() = b'Z'; // same length, still valid JSON string interior
        let mut reader = ReplayReader::new(raw.as_slice(), &plan).unwrap();
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        assert_eq!(output, expected);
        assert!(
            reader.finish().is_err(),
            "depth {depth}: leaf digest does not prove parent tail"
        );
    }
}

#[test]
fn every_layer_digest_is_checked_even_when_outer_and_leaf_digests_match() {
    let (raw, plan, expected) = nested(4, CHUNK * 2);
    for target in 0..4 {
        let mut ranges = plan.ranges().to_vec();
        ranges[target].decoded_sha1[0] ^= 1;
        let changed = DecodePlan::new(ranges).unwrap();
        let mut reader = ReplayReader::new(raw.as_slice(), &changed).unwrap();
        let mut output = Vec::new();
        let read = reader.read_to_end(&mut output);
        if read.is_ok() {
            assert_eq!(output, expected);
        }
        assert!(reader.finish().is_err(), "layer {target} must be verified");
    }
}

#[test]
fn final_layer_must_reach_eof_before_finish_even_with_exact_output_count() {
    let (raw, plan, expected) = nested(3, 10);
    let mut reader = ReplayReader::new(raw.as_slice(), &plan).unwrap();
    reader.read_exact(&mut vec![0; expected.len()]).unwrap();
    assert!(reader.finish().is_err());
    let mut reader = ReplayReader::new(raw.as_slice(), &plan).unwrap();
    assert_eq!(reader.read(&mut []).unwrap(), 0);
    assert!(reader.finish().is_err());
}

#[test]
fn empty_target_still_verifies_skipped_prefix_and_parent_tail() {
    let text = b"prefix\"\"tail";
    let raw = escaped(text);
    let plan = DecodePlan::new(vec![range(0, &raw, text), range(7, b"", b"")]).unwrap();
    let mut reader = ReplayReader::new(raw.as_slice(), &plan).unwrap();
    assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    assert!(reader.finish().unwrap().is_empty());
    let plan = DecodePlan::new(vec![range(0, b"", b"")]).unwrap();
    let mut reader = ReplayReader::new(b"".as_slice(), &plan).unwrap();
    assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    reader.finish().unwrap();
}

#[test]
fn short_or_extra_outer_source_cannot_be_accepted_as_an_exact_range() {
    let raw = b"abc";
    let plan = DecodePlan::new(vec![range(900, raw, raw)]).unwrap();
    let mut short = ReplayReader::new(b"ab".as_slice(), &plan).unwrap();
    assert!(short.read_to_end(&mut Vec::new()).is_err());
    assert!(short.read(&mut [0; 1]).is_err());
    assert!(short.finish().is_err());
    let mut extra = ReplayReader::new(b"abcd".as_slice(), &plan).unwrap();
    let mut output = Vec::new();
    extra.read_to_end(&mut output).unwrap();
    assert_eq!(output, raw);
    assert!(extra.finish().is_err());
}

#[test]
fn underlying_failure_is_sanitized_and_reader_failure_is_sticky() {
    struct Once {
        failed: bool,
    }
    impl Read for Once {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            if self.failed {
                panic!("underlying source retried after hard failure");
            }
            self.failed = true;
            Err(io::Error::other("SECRET actual path"))
        }
    }
    let plan = DecodePlan::new(vec![range(0, b"abc", b"abc")]).unwrap();
    let mut reader = ReplayReader::new(Once { failed: false }, &plan).unwrap();
    let error = reader.read_to_end(&mut Vec::new()).unwrap_err();
    assert!(!error.to_string().contains("SECRET"));
    assert!(reader.read(&mut [0; 1]).is_err());
    assert!(reader.finish().is_err());
}

#[test]
fn generated_multimegabyte_source_has_fixed_read_requests_and_no_body_buffer() {
    struct Generated {
        left: usize,
        maximum: usize,
    }
    impl Read for Generated {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.maximum = self.maximum.max(output.len());
            let count = self.left.min(output.len());
            output[..count].fill(b'A');
            self.left -= count;
            Ok(count)
        }
    }
    let len = 4 * 1024 * 1024;
    let mut hash = Sha1::new();
    for _ in 0..len / CHUNK {
        hash.update([b'A'; CHUNK]);
    }
    let plan = DecodePlan::new(vec![StringRange {
        start: 800,
        end: 800 + len as u64,
        decoded_len: len as u64,
        decoded_sha1: hash.finalize().into(),
    }])
    .unwrap();
    let mut reader = ReplayReader::new(
        Generated {
            left: len,
            maximum: 0,
        },
        &plan,
    )
    .unwrap();
    assert!(std::mem::size_of_val(&reader) < 256);
    assert_eq!(io::copy(&mut reader, &mut io::sink()).unwrap(), len as u64);
    let source = reader.finish().unwrap();
    assert_eq!(source.left, 0);
    assert!(source.maximum <= CHUNK);
}

#[test]
fn multimegabyte_parent_skip_and_tail_are_streamed() {
    struct Generated {
        left: usize,
    }
    impl Read for Generated {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            assert!(output.len() <= CHUNK);
            let count = self.left.min(output.len());
            output[..count].fill(b'A');
            self.left -= count;
            Ok(count)
        }
    }
    let length = 4 * 1024 * 1024;
    let mut parent_hash = Sha1::new();
    for _ in 0..length / CHUNK {
        parent_hash.update([b'A'; CHUNK]);
    }
    let plan = DecodePlan::new(vec![
        StringRange {
            start: 0,
            end: length as u64,
            decoded_len: length as u64,
            decoded_sha1: parent_hash.finalize().into(),
        },
        range((length / 2) as u64, b"AAAA", b"AAAA"),
    ])
    .unwrap();
    let mut reader = ReplayReader::new(Generated { left: length }, &plan).unwrap();
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"AAAA");
    assert_eq!(reader.finish().unwrap().left, 0);
}

#[test]
fn large_range_validation_does_not_preallocate_its_bytes() {
    let plan = DecodePlan::new(vec![StringRange {
        start: 0,
        end: u64::MAX / 2,
        decoded_len: u64::MAX / 2,
        decoded_sha1: hash(b"synthetic"),
    }])
    .unwrap();
    assert_eq!(plan.ranges().len(), 1);
    assert!(plan.resident_len() < 1024);
}
