use super::*;
use std::io::Cursor;

struct Chunked<'a> {
    bytes: &'a [u8],
    chunk: usize,
    interrupted: bool,
}
impl Read for Chunked<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.interrupted {
            self.interrupted = false;
            return Err(io::ErrorKind::Interrupted.into());
        }
        let count = self.bytes.len().min(self.chunk).min(output.len());
        output[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}
fn hash(bytes: &[u8]) -> [u8; 20] {
    Sha1::digest(bytes).into()
}
fn decode(raw: &[u8], expected: &[u8], input_chunk: usize, output_chunk: usize) {
    let source = Chunked {
        bytes: raw,
        chunk: input_chunk,
        interrupted: true,
    };
    let mut reader = JsonStringReader::new(
        source,
        expected.len() as u64,
        hash(expected),
        raw.len() as u64,
    )
    .unwrap();
    let mut output = vec![0; output_chunk];
    let mut received = Vec::new();
    loop {
        let count = reader.read(&mut output).unwrap();
        if count == 0 {
            break;
        }
        received.extend_from_slice(&output[..count]);
    }
    assert_eq!(received, expected);
    assert!(reader.finish().unwrap().bytes.is_empty());
}

#[test]
fn all_escapes_utf8_and_surrogates_match_serde_across_every_small_chunk() {
    let spellings = [
        "",
        r#"plain / ASCII and DEL "#,
        r#"\"\\\/\b\f\n\r\t\u0000\u007f"#,
        r#"a\/é中文😀\u00e9\u4e2D\uD83D\uDe00\uDBFF\uDFFF"#,
    ];
    for raw in spellings {
        let quoted = format!("\"{raw}\"");
        let expected: String = serde_json::from_str(&quoted).unwrap();
        for input in [1, 2, 3, 7, BUFFER] {
            for output in [1, 2, 3, 4, 17, BUFFER] {
                decode(raw.as_bytes(), expected.as_bytes(), input, output);
            }
        }
    }
}

#[test]
fn long_ascii_and_escape_at_internal_buffer_edge_have_identical_digest() {
    for boundary in [BUFFER - 3, BUFFER - 1, BUFFER, BUFFER + 1] {
        let raw = format!(
            "{}\\uD83D\\uDE00/{}",
            "x".repeat(boundary),
            "a".repeat(BUFFER * 2)
        );
        let expected = format!("{}😀/{}", "x".repeat(boundary), "a".repeat(BUFFER * 2));
        decode(raw.as_bytes(), expected.as_bytes(), BUFFER, BUFFER + 11);
    }
}

#[test]
fn malformed_utf8_controls_quotes_escapes_and_surrogates_fail_closed() {
    let invalid_inputs: &[&[u8]] = &[
        b"\"",
        b"a\nb",
        b"\x00",
        b"\x1f",
        b"\\",
        b"\\x",
        b"\\u",
        b"\\u000",
        b"\\u00x0",
        b"\\uDC00",
        b"\\uD800",
        b"\\uD800x",
        b"\\uD800\\n",
        b"\\uD800\\uD800",
        b"\\uD800\\u0041",
        b"\x80",
        b"\xc0\x80",
        b"\xc2",
        b"\xc2x",
        b"\xe0\x80\x80",
        b"\xed\xa0\x80",
        b"\xf0\x80\x80\x80",
        b"\xf4\x90\x80\x80",
        b"\xf5\x80\x80\x80",
        b"\xff",
        b"\xf0\x9f\x98",
    ];
    for raw in invalid_inputs {
        for chunk in [1, 2, BUFFER] {
            let source = Chunked {
                bytes: raw,
                chunk,
                interrupted: false,
            };
            let mut reader =
                JsonStringReader::new(source, raw.len() as u64, hash(raw), raw.len() as u64)
                    .unwrap();
            let error = reader.read_to_end(&mut Vec::new()).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{raw:?}");
            assert!(reader.read(&mut [0; 8]).is_err(), "errors are sticky");
            assert!(reader.finish().is_err());
        }
    }
}

#[test]
fn every_proper_truncation_of_an_escape_or_multibyte_scalar_is_invalid() {
    for raw in [b"\\uD83D\\uDE00".as_slice(), "😀".as_bytes(), b"\\u0041"] {
        for length in 1..raw.len() {
            let mut reader =
                JsonStringReader::new(&raw[..length], length as u64, [0; 20], length as u64)
                    .unwrap();
            assert!(reader.read_to_end(&mut Vec::new()).is_err());
            assert!(reader.finish().is_err());
        }
    }
}

#[test]
fn expected_length_and_hash_are_both_required_before_successful_eof() {
    for (expected_len, digest) in [(2, hash(b"abc")), (4, hash(b"abc")), (3, hash(b"abd"))] {
        let mut reader = JsonStringReader::new(b"abc".as_slice(), expected_len, digest, 4).unwrap();
        assert_eq!(
            reader.read_to_end(&mut Vec::new()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(reader.finish().is_err());
    }
    decode(b"a\\/b", b"a/b", 2, 2);
    decode(b"a/b", b"a/b", 2, 2);
}

#[test]
fn partial_consumption_or_empty_output_does_not_fake_verified_eof() {
    let mut reader = JsonStringReader::new(b"abc".as_slice(), 3, hash(b"abc"), 3).unwrap();
    assert_eq!(reader.read(&mut []).unwrap(), 0);
    assert!(reader.finish().is_err());
    let mut reader = JsonStringReader::new(b"abc".as_slice(), 3, hash(b"abc"), 3).unwrap();
    reader.read_exact(&mut [0; 3]).unwrap();
    assert!(
        reader.finish().is_err(),
        "exact output length is not source EOF"
    );
    let reader = JsonStringReader::new(b"".as_slice(), 0, hash(b""), 0).unwrap();
    assert!(reader.finish().is_err());
    decode(b"", b"", 1, 1);
}

#[test]
fn physical_limit_probes_only_one_extra_byte_and_never_succeeds_truncated() {
    let mut source = Cursor::new(b"abcdef".as_slice());
    let mut reader = JsonStringReader::new(&mut source, 3, hash(b"abc"), 3).unwrap();
    assert_eq!(
        reader.read_to_end(&mut Vec::new()).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(reader.finish().is_err());
    assert_eq!(source.position(), 4);
    assert!(JsonStringReader::new(b"".as_slice(), 2, hash(b""), 1).is_err());
    assert!(JsonStringReader::new(b"".as_slice(), 0, hash(b""), MAX_PHYSICAL + 1).is_err());
    decode(b"abc", b"abc", 1, 3);
}

#[test]
fn underlying_error_is_sanitized_and_cannot_be_retried_into_success() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("SECRET source path and payload"))
        }
    }
    let mut reader = JsonStringReader::new(Broken, 0, hash(b""), 0).unwrap();
    let error = reader.read(&mut [0; 1]).unwrap_err();
    assert!(!error.to_string().contains("SECRET"));
    assert!(reader.finish().is_err());
}

#[test]
fn nested_string_readers_require_completion_of_each_layer() {
    let inner_raw = br"a\u002fb";
    let quoted_outer = serde_json::to_vec(std::str::from_utf8(inner_raw).unwrap()).unwrap();
    let physical = &quoted_outer[1..quoted_outer.len() - 1];
    let outer = JsonStringReader::new(
        physical,
        inner_raw.len() as u64,
        hash(inner_raw),
        physical.len() as u64,
    )
    .unwrap();
    let mut inner = JsonStringReader::new(outer, 3, hash(b"a/b"), inner_raw.len() as u64).unwrap();
    let mut value = Vec::new();
    inner.read_to_end(&mut value).unwrap();
    assert_eq!(value, b"a/b");
    let source = inner.finish().unwrap().finish().unwrap();
    assert!(source.is_empty());
}

#[test]
fn scanner_physical_span_length_and_digest_are_the_reader_contract() {
    use crate::sessions::records::scanner::{self, Node, Text};
    for document in [
        "  \"a/é😀\"\n",
        "  \"\\u0061\\/\\u00e9\\ud83d\\ude00\"\n",
        "\"data:image\\/png;base64,YWJj\"\n",
    ] {
        let parsed = scanner::scan(
            Chunked {
                bytes: document.as_bytes(),
                chunk: 1,
                interrupted: false,
            },
            scanner::Limits {
                inline_string_bytes: 2,
                ..Default::default()
            },
        )
        .unwrap();
        let Node::String(Text::Span(span)) = parsed.root else {
            panic!("expected private span")
        };
        let physical = &document.as_bytes()[span.start() as usize..span.end() as usize];
        let mut reader = JsonStringReader::new(
            physical,
            span.decoded_len(),
            *span.digest(),
            span.end() - span.start(),
        )
        .unwrap();
        let mut output = String::new();
        reader.read_to_string(&mut output).unwrap();
        let expected: String = serde_json::from_str(document).unwrap();
        assert_eq!(output, expected);
        assert!(reader.finish().unwrap().is_empty());
    }
}

#[test]
fn generated_large_base64_like_input_uses_only_fixed_reader_buffers() {
    struct Generated {
        remaining: u64,
        maximum_request: usize,
    }
    impl Read for Generated {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            self.maximum_request = self.maximum_request.max(output.len());
            let count = self.remaining.min(output.len() as u64) as usize;
            output[..count].fill(b'A');
            self.remaining -= count as u64;
            Ok(count)
        }
    }
    let length = 4 * 1024 * 1024;
    let mut expected = Sha1::new();
    for _ in 0..length / BUFFER {
        expected.update([b'A'; BUFFER]);
    }
    let source = Generated {
        remaining: length as u64,
        maximum_request: 0,
    };
    let mut reader = JsonStringReader::new(
        source,
        length as u64,
        expected.finalize().into(),
        length as u64,
    )
    .unwrap();
    assert!(std::mem::size_of_val(&reader) < BUFFER + 1024);
    assert_eq!(
        io::copy(&mut reader, &mut io::sink()).unwrap(),
        length as u64
    );
    let source = reader.finish().unwrap();
    assert_eq!(source.remaining, 0);
    assert!(source.maximum_request <= BUFFER);
}

#[test]
fn checked_native_range_and_string_hash_must_both_finish_before_publication() {
    use crate::sessions::{native_input::CheckedNative, stamp};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("synthetic.jsonl");
    let bytes = br#"{"image":"Y\u0057Jj","tail":"unchanged"}"#;
    std::fs::write(&path, bytes).unwrap();
    let expected = stamp(&path).unwrap();
    let range = CheckedNative::open_range(&root, &path, &expected, 10, 19).unwrap();
    let mut reader = JsonStringReader::new(range, 4, hash(b"YWJj"), 9).unwrap();
    let mut output = Vec::new();
    reader.read_to_end(&mut output).unwrap();
    assert_eq!(output, b"YWJj");
    let range = reader.finish().unwrap();
    range.finish().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);

    let range = CheckedNative::open_range(&root, &path, &expected, 10, 19).unwrap();
    let mut reader = JsonStringReader::new(range, 4, hash(b"YWJj"), 9).unwrap();
    reader.read_to_end(&mut Vec::new()).unwrap();
    let range = reader.finish().unwrap();
    let mut replacement = bytes.to_vec();
    *replacement.last_mut().unwrap() = b' ';
    std::fs::write(&path, replacement).unwrap();
    assert!(
        range.finish().is_err(),
        "unread suffix changes also invalidate verified string bytes"
    );
}
