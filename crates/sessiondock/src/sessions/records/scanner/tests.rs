use super::*;
use std::io::{self, Cursor};

#[test]
fn span_data_url_prefix_and_payload_evidence_survive_all_chunk_boundaries() {
    let payload = "A/é".repeat(9000);
    let decoded = format!("data:image/png;base64,{payload}");
    let encoded = serde_json::to_string(&decoded)
        .unwrap()
        .replace('/', "\\/")
        .replace('é', "\\u00e9");
    for chunk in [1, 2, 3, 5, 21, 22, 23, 255, 8192] {
        for threshold in [0, 8, 22, 64, 9000] {
            let document = scan(
                Chunked::new(encoded.as_bytes(), chunk),
                Limits {
                    inline_string_bytes: threshold,
                    ..Default::default()
                },
            )
            .unwrap();
            let span = span(&document);
            assert_eq!(
                span.digest(),
                &<[u8; 20]>::from(Sha1::digest(decoded.as_bytes()))
            );
            assert_eq!(span.prefix(), &decoded.as_bytes()[..256]);
            assert_eq!(
                span.data_suffix(),
                Some((22, Sha1::digest(payload.as_bytes()).into()))
            );
            assert_eq!(span.decoded_len(), decoded.len() as u64);
            assert!(span.escaped());
        }
    }
}

#[test]
fn span_prefix_is_bounded_and_data_candidate_is_not_media_validation() {
    for (text, expected) in [
        ("ordinary,".repeat(100), None),
        (format!("data:{},AAAA", "x".repeat(300)), None),
        ("data:unsupported,AAAA".repeat(100), Some(17)),
    ] {
        let encoded = serde_json::to_vec(&text).unwrap();
        let document = scan(
            encoded.as_slice(),
            Limits {
                inline_string_bytes: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(span(&document).prefix().len() <= 256);
        assert_eq!(
            span(&document).data_suffix().map(|(offset, _)| offset),
            expected
        );
        assert!(document.into_value().is_err());
    }
}

struct Chunked<R> {
    reader: R,
    size: usize,
    calls: usize,
    bytes: usize,
    maximum_request: usize,
}
impl<R> Chunked<R> {
    fn new(reader: R, size: usize) -> Self {
        Self {
            reader,
            size,
            calls: 0,
            bytes: 0,
            maximum_request: 0,
        }
    }
}
impl<R: Read> Read for Chunked<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.maximum_request = self.maximum_request.max(output.len());
        let count = output.len().min(self.size);
        let read = self.reader.read(&mut output[..count])?;
        self.calls += 1;
        self.bytes += read;
        Ok(read)
    }
}
fn value(bytes: &[u8], chunk: usize) -> Value {
    let expected = scan(Chunked::new(bytes, chunk), Limits::default())
        .unwrap()
        .into_value()
        .unwrap();
    let direct = scan_value(Chunked::new(bytes, chunk), Limits::default()).unwrap();
    assert_eq!(
        serde_json::to_vec(&direct).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    direct
}
fn failure(bytes: &[u8], limits: Limits) -> ScanError {
    let expected = scan(Chunked::new(bytes, 3), limits)
        .err()
        .expect("must reject");
    assert_eq!(
        scan_value(Chunked::new(bytes, 3), limits).unwrap_err(),
        expected
    );
    expected
}
fn span(document: &Document) -> &TextSpan {
    let Node::String(Text::Span(span)) = &document.root else {
        panic!("expected unauthorized text span");
    };
    span
}

#[test]
fn every_byte_can_split_json_tokens_utf8_and_surrogate_pairs() {
    let input = br#" {"\u006bey": [null, true, false, -0, 1.25e-12, 18446744073709551615], "escaped":"a\"\\\/\b\f\n\r\t\u0000\u00e9\ud83d\ude00", "nested":{"x":[]}} \n"#;
    // The final suffix here intentionally uses JSON whitespace, not a literal
    // backslash-n outside a string.
    let mut bytes = input[..input.len() - 2].to_vec();
    bytes.extend_from_slice(b"\n\t\r ");
    let expected: Value = serde_json::from_slice(&bytes).unwrap();
    for chunk in 1..=64 {
        assert_eq!(value(&bytes, chunk), expected, "chunk {chunk}");
    }
    let raw = "{\"中文🔒\":\"é\u{10ffff}💡\"}";
    for chunk in 1..=raw.len() {
        assert_eq!(
            value(raw.as_bytes(), chunk),
            serde_json::from_str::<Value>(raw).unwrap()
        );
    }
}

#[test]
fn duplicate_decoded_keys_rejected_but_nested_reuse_is_valid() {
    for input in [
        r#"{"a":1,"a":2}"#,
        r#"{"a":1,"\u0061":2}"#,
        r#"{"\ud83d\ude00":0,"😀":1}"#,
        r#"{"x":{"a":0,"a":0}}"#,
    ] {
        assert_eq!(
            failure(input.as_bytes(), Limits::default()).kind,
            ErrorKind::DuplicateKey
        );
    }
    let input = br#"{"a":1,"nested":{"a":2},"A":3}"#;
    assert_eq!(
        value(input, 1),
        serde_json::from_slice::<Value>(input).unwrap()
    );
    let error = failure(br#"{"secret-input":0,"secret-input":1}"#, Limits::default());
    assert!(!error.to_string().contains("secret"));
    assert!(!format!("{error:?}").contains("secret"));
}

#[test]
fn object_insertion_order_survives_nested_scanning_and_value_serialization() {
    // Value equality intentionally ignores object order; this contract must
    // compare serialized spelling and iterator order used by tool summaries.
    let input = br#"{"z":"first","a":false,"r":4,"b":"last","nested":{"zz":1,"\u0061a":2,"mm":3}}"#;
    let expected: Value = serde_json::from_slice(input).unwrap();
    let expected_bytes = serde_json::to_vec(&expected).unwrap();
    for chunk in [1, 2, 7, BUFFER] {
        let document = scan(Chunked::new(input.as_slice(), chunk), Limits::default()).unwrap();
        let actual = document.into_value().unwrap();
        assert_eq!(serde_json::to_vec(&actual).unwrap(), expected_bytes);
        assert_eq!(
            actual
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["z", "a", "r", "b", "nested"]
        );
    }
    let input = (0..5000)
        .rev()
        .map(|i| format!("\"field-{i:04}\":{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let input = format!("{{{input}}}");
    let document = scan(
        input.as_bytes(),
        Limits {
            resident_bytes: 8 * 1024 * 1024,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(document.stats.keys, 5000);
    let actual = document.into_value().unwrap();
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&serde_json::from_str::<Value>(&input).unwrap()).unwrap()
    );
}

#[test]
fn malformed_grammar_trailing_data_and_partial_records_never_succeed() {
    let invalid = [
        "",
        " ",
        "nul",
        "True",
        "NaN",
        "Infinity",
        "+1",
        ".1",
        "01",
        "-01",
        "1.",
        "1e",
        "1e+",
        "--1",
        "[1,]",
        "{\"a\":1,}",
        "{a:1}",
        "{\"a\" 1}",
        "[1 2]",
        "{}{}",
        "null false",
        "/*x*/null",
        "\"raw\nline\"",
        "\"\\x20\"",
        "\"\\u123\"",
        "\"\\ud800\"",
        "\"\\udc00\"",
        "\"\\ud800\\u0041\"",
        "\"unterminated",
        "[",
        "{",
        "[null",
        "{\"a\":",
    ];
    for input in invalid {
        for chunk in [1, 7, 8192] {
            assert!(
                scan(Chunked::new(input.as_bytes(), chunk), Limits::default()).is_err(),
                "accepted {input:?} at chunk {chunk}"
            );
            assert!(scan_value(Chunked::new(input.as_bytes(), chunk), Limits::default()).is_err());
        }
    }
    assert_eq!(value(b"null\r\n", 1), Value::Null);
    assert!(scan(b"null\x0b".as_slice(), Limits::default()).is_err());
    assert!(scan(b"\xef\xbb\xbfnull".as_slice(), Limits::default()).is_err());
}

#[test]
fn invalid_utf8_is_checked_even_after_string_has_become_a_span() {
    let bad: &[&[u8]] = &[
        b"\x80",
        b"\xc0\xaf",
        b"\xe0\x80\xaf",
        b"\xed\xa0\x80",
        b"\xf4\x90\x80\x80",
        b"\xf5\x80\x80\x80",
        b"\xe2\x82",
        b"\xff",
    ];
    for suffix in bad {
        let mut input = b"\"prefix".to_vec();
        input.extend_from_slice(suffix);
        input.push(b'"');
        for chunk in [1, 2, 8, 8192] {
            let error = scan(
                Chunked::new(input.as_slice(), chunk),
                Limits {
                    inline_string_bytes: 1,
                    ..Limits::default()
                },
            )
            .err()
            .unwrap();
            assert_eq!(error.kind, ErrorKind::Utf8);
        }
    }
    let mut input = b"\"prefix".to_vec();
    input.extend_from_slice(b"\\ud800\\u0041\"");
    assert_eq!(
        failure(
            &input,
            Limits {
                inline_string_bytes: 1,
                ..Limits::default()
            }
        )
        .kind,
        ErrorKind::Syntax
    );
}

#[test]
fn span_offsets_exclude_quotes_and_digest_covers_unescaped_utf8_not_spelling() {
    let raw = "  \"a/é😀\"\n";
    let escaped = "  \"\\u0061\\/\\u00e9\\ud83d\\ude00\"\n";
    let limits = Limits {
        inline_string_bytes: 2,
        ..Limits::default()
    };
    let a = scan(Chunked::new(raw.as_bytes(), 1), limits).unwrap();
    let b = scan(Chunked::new(escaped.as_bytes(), 1), limits).unwrap();
    let expected: [u8; 20] = Sha1::digest("a/é😀".as_bytes()).into();
    assert_eq!(span(&a).start(), 3);
    assert_eq!(span(&a).end(), raw.len() as u64 - 2);
    assert_eq!(
        &raw.as_bytes()[span(&a).start() as usize..span(&a).end() as usize],
        "a/é😀".as_bytes()
    );
    assert_eq!(
        &escaped.as_bytes()[span(&b).start() as usize..span(&b).end() as usize],
        b"\\u0061\\/\\u00e9\\ud83d\\ude00"
    );
    assert_eq!(span(&a).decoded_len(), "a/é😀".len() as u64);
    assert_eq!(span(&a).digest(), &expected);
    assert_eq!(span(&a).digest(), span(&b).digest());
    assert!(!span(&a).escaped());
    assert!(span(&b).escaped());
    assert_eq!(a.stats.span_count, 1);
    assert_eq!(a.stats.physical_bytes, raw.len() as u64);
    assert_eq!(
        a.into_value().err().unwrap().kind,
        ErrorKind::UnmaterializedSpan
    );
}

#[test]
fn inline_threshold_is_decoded_utf8_bytes_and_strict_small_records_remain_movable() {
    let mut build = StringBuild::new();
    let mut budget = Resident {
        maximum: 64,
        used: 0,
        peak: 0,
    };
    build.feed(b"small", 64, false, &mut budget, 0).unwrap();
    assert!(
        build.hash.is_none(),
        "resident text does not initialize a digest buffer"
    );
    for input in ["\"éé\"", "\"\\u00e9\\u00e9\""] {
        let document = scan(
            input.as_bytes(),
            Limits {
                inline_string_bytes: 4,
                ..Limits::default()
            },
        )
        .unwrap();
        assert_eq!(document.stats.span_count, 0);
        assert_eq!(document.into_value().unwrap(), "éé");
        assert!(matches!(
            scan(
                input.as_bytes(),
                Limits {
                    inline_string_bytes: 3,
                    ..Limits::default()
                }
            )
            .unwrap()
            .root,
            Node::String(Text::Span(_))
        ));
    }
    let text = "x".repeat(1024 * 1024);
    let input = format!("{{\"text\":\"{text}\"}}");
    let document = scan(
        Chunked::new(input.as_bytes(), 701),
        Limits {
            inline_string_bytes: 2 * 1024 * 1024,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_eq!(document.stats.span_count, 0);
    let Node::Object(mut object) = document.root else {
        panic!("object");
    };
    let Node::String(Text::Inline(string)) = object.shift_remove("text").unwrap() else {
        panic!("inline");
    };
    let pointer = string.as_ptr();
    let moved = Node::String(Text::Inline(string)).into_value().unwrap();
    assert_eq!(moved.as_str().unwrap().as_ptr(), pointer);
    assert_eq!(moved, text);
}

#[test]
fn huge_base64_shaped_source_streams_without_a_giant_resident_string() {
    // Generate the physical base64 spelling of 32 MiB without constructing a
    // giant fixture String/Vec, or asking the scanner to classify it as media.
    let decoded_image = 32 * 1024 * 1024u64;
    let encoded = decoded_image.div_ceil(3) * 4;
    let source = Cursor::new(b"\"")
        .chain(io::repeat(b'A').take(encoded - 1))
        .chain(Cursor::new(b"=\""));
    let mut reader = Chunked::new(source, 1021);
    let document = scan(&mut reader, Limits::default()).unwrap();
    let span = span(&document);
    assert_eq!(span.start(), 1);
    assert_eq!(span.end(), encoded + 1);
    assert_eq!(span.decoded_len(), encoded); // NOT the decoded-image byte count.
    assert!(!span.escaped());
    let mut expected = Sha1::new();
    let repeated = [b'A'; 8192];
    let mut remaining = encoded - 1;
    while remaining != 0 {
        let count = remaining.min(repeated.len() as u64) as usize;
        expected.update(&repeated[..count]);
        remaining -= count as u64;
    }
    expected.update(b"=");
    assert_eq!(span.digest(), &<[u8; 20]>::from(expected.finalize()));
    assert_eq!(reader.bytes as u64, encoded + 2);
    assert!(reader.calls > 30_000);
    assert!(reader.maximum_request <= BUFFER);
    assert!(document.stats.peak_resident_bytes < 80 * 1024);
    assert_eq!(document.stats.resident_bytes, NODE_WEIGHT + 256);
    assert_eq!(
        document.into_value().err().unwrap().kind,
        ErrorKind::UnmaterializedSpan
    );
}

#[test]
fn escaped_stream_hash_covers_discarded_prefix_middle_and_tail() {
    let content = "\\u0041".repeat(32_769);
    let input = format!("\"{content}\\u0000\\ud83d\\ude00\"");
    let document = scan(
        Chunked::new(input.as_bytes(), 11),
        Limits {
            inline_string_bytes: 31,
            ..Limits::default()
        },
    )
    .unwrap();
    let mut expected = Sha1::new();
    expected.update(vec![b'A'; 32_769]);
    expected.update("\0😀".as_bytes());
    assert_eq!(
        span(&document).digest(),
        &<[u8; 20]>::from(expected.finalize())
    );
    assert_eq!(span(&document).decoded_len(), 32_769 + 5);
    assert!(span(&document).escaped());
    let mut changed = input.into_bytes();
    let middle = 1 + 6 * 12_000 + 5;
    changed[middle] = b'2'; // One decoded byte well after the retained prefix.
    let other = scan(
        changed.as_slice(),
        Limits {
            inline_string_bytes: 31,
            ..Limits::default()
        },
    )
    .unwrap();
    assert_ne!(span(&document).digest(), span(&other).digest());
}

#[test]
fn physical_budget_counts_whitespace_and_escapes_and_reads_only_one_probe_byte() {
    for input in [
        b"null".as_slice(),
        b" null\r\n".as_slice(),
        br#""\u0061""#.as_slice(),
    ] {
        let maximum = input.len() as u64;
        assert!(
            scan(
                input,
                Limits {
                    physical_bytes: maximum,
                    ..Limits::default()
                }
            )
            .is_ok()
        );
        let mut reader = Chunked::new(input, 8192);
        assert_eq!(
            scan(
                &mut reader,
                Limits {
                    physical_bytes: maximum - 1,
                    ..Limits::default()
                }
            )
            .err()
            .unwrap()
            .kind,
            ErrorKind::PhysicalLimit
        );
        assert!(reader.bytes <= maximum as usize);
    }
    let mut endless = Chunked::new(io::repeat(b' '), 3);
    assert_eq!(
        scan(
            &mut endless,
            Limits {
                physical_bytes: 19,
                ..Limits::default()
            }
        )
        .err()
        .unwrap()
        .kind,
        ErrorKind::PhysicalLimit
    );
    assert_eq!(endless.bytes, 20);
}

#[test]
fn depth_node_key_and_resident_budgets_fail_closed_at_exact_boundaries() {
    let deepest = format!("{}null{}", "[".repeat(HARD_DEPTH), "]".repeat(HARD_DEPTH));
    assert!(
        scan(
            deepest.as_bytes(),
            Limits {
                depth: HARD_DEPTH,
                ..Limits::default()
            }
        )
        .unwrap()
        .into_value()
        .is_ok()
    );
    let depth = format!("{}null{}", "[".repeat(8), "]".repeat(8));
    let limits = Limits {
        depth: 8,
        ..Limits::default()
    };
    assert!(scan(depth.as_bytes(), limits).is_ok());
    assert_eq!(
        failure(format!("[{depth}]").as_bytes(), limits).kind,
        ErrorKind::DepthLimit
    );
    assert_eq!(
        failure(
            b"null",
            Limits {
                depth: 129,
                ..limits
            }
        )
        .kind,
        ErrorKind::InvalidLimits
    );
    let hostile = io::repeat(b'[').take(100_000);
    assert_eq!(
        scan(hostile, limits).err().unwrap().kind,
        ErrorKind::DepthLimit
    );
    assert!(scan(b"[null]".as_slice(), Limits { nodes: 2, ..limits }).is_ok());
    assert_eq!(
        failure(b"[null,null]", Limits { nodes: 2, ..limits }).kind,
        ErrorKind::NodeLimit
    );
    assert!(scan(br#"{"a":{"b":0}}"#.as_slice(), Limits { keys: 2, ..limits }).is_ok());
    assert_eq!(
        failure(br#"{"a":{"b":0}}"#, Limits { keys: 1, ..limits }).kind,
        ErrorKind::KeyLimit
    );
    assert!(
        scan(
            b"null".as_slice(),
            Limits {
                resident_bytes: NODE_WEIGHT,
                ..limits
            }
        )
        .is_ok()
    );
    assert_eq!(
        failure(
            b"null",
            Limits {
                resident_bytes: NODE_WEIGHT - 1,
                ..limits
            }
        )
        .kind,
        ErrorKind::ResidentLimit
    );
    let input = br#"{"a":["short",1,false],"b":"text"}"#;
    let stats = scan(input.as_slice(), limits).unwrap().stats;
    assert!(
        scan(
            input.as_slice(),
            Limits {
                resident_bytes: stats.peak_resident_bytes,
                ..limits
            }
        )
        .is_ok()
    );
    assert_eq!(
        failure(
            input,
            Limits {
                resident_bytes: stats.peak_resident_bytes - 1,
                ..limits
            }
        )
        .kind,
        ErrorKind::ResidentLimit
    );
}

#[test]
fn keys_never_become_spans_and_numeric_bounds_preserve_serde_behavior() {
    let limits = Limits {
        key_bytes: 3,
        number_bytes: 3,
        ..Limits::default()
    };
    assert_eq!(
        scan(br#"{"\u0061bc":123}"#.as_slice(), limits)
            .unwrap()
            .into_value()
            .unwrap(),
        serde_json::json!({"abc":123})
    );
    assert_eq!(
        failure(br#"{"abcd":1}"#, limits).kind,
        ErrorKind::StringLimit
    );
    assert_eq!(failure(b"1234", limits).kind, ErrorKind::NumberLimit);
    for number in [
        "-0",
        "0.0",
        "1e-999",
        "-9223372036854775808",
        "18446744073709551615",
        "18446744073709551616",
        "1.7976931348623157e308",
        "1e309",
        "-1e400",
    ] {
        let expected = serde_json::from_str::<Value>(number);
        let actual = scan(number.as_bytes(), Limits::default()).and_then(Document::into_value);
        match expected {
            Ok(value) => assert_eq!(actual.unwrap(), value, "{number}"),
            Err(_) => assert!(actual.is_err(), "{number}"),
        }
    }
}

#[test]
fn unknown_nested_tool_strings_are_private_spans_not_fake_json_or_media() {
    let input = br#"{"output":"{\"content\":[{\"type\":\"image\",\"data\":\"AAAAAAAA\"}]}"}"#;
    let document = scan(
        input.as_slice(),
        Limits {
            inline_string_bytes: 8,
            ..Limits::default()
        },
    )
    .unwrap();
    let Node::Object(object) = &document.root else {
        panic!("object");
    };
    assert!(matches!(
        object.get("output"),
        Some(Node::String(Text::Span(_)))
    ));
    assert_eq!(object.len(), 1);
    assert_eq!(document.stats.nodes, 2);
    assert_eq!(
        document.into_value().err().unwrap().kind,
        ErrorKind::UnmaterializedSpan
    );
}

#[test]
fn read_failures_and_interrupted_reads_do_not_leak_input_or_return_partial_tree() {
    struct FailAfter {
        prefix: Cursor<&'static [u8]>,
        interrupted: bool,
    }
    impl Read for FailAfter {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = self.prefix.read(bytes)?;
            if count != 0 {
                return Ok(count);
            }
            Err(io::Error::other("credential-like-private-reader-error"))
        }
    }
    let error = scan(
        FailAfter {
            prefix: Cursor::new(b"{\"data\":\"long string"),
            interrupted: false,
        },
        Limits {
            inline_string_bytes: 1,
            ..Limits::default()
        },
    )
    .err()
    .unwrap();
    assert_eq!(error.kind, ErrorKind::Io);
    assert!(!format!("{error} {error:?}").contains("credential"));
    // A bounded single-record input leaves the following record untouched.
    let mut reader = Cursor::new(b"nulltrue");
    assert_eq!(
        scan((&mut reader).take(4), Limits::default())
            .unwrap()
            .into_value()
            .unwrap(),
        Value::Null
    );
    assert_eq!(reader.position(), 4);
    assert_eq!(
        scan(&mut reader, Limits::default())
            .unwrap()
            .into_value()
            .unwrap(),
        true
    );
}

fn cpu_cases() -> Vec<(&'static str, Vec<Vec<u8>>, usize)> {
    let native = [
        include_bytes!("../../../../tests/fixtures/claude/project-demo/synthetic-claude.jsonl")
            .as_slice(),
        include_bytes!("../../../../tests/fixtures/codex/2026/09/11/rollout-synthetic-codex.jsonl")
            .as_slice(),
        include_bytes!(
            "../../../../tests/fixtures/grok/project-demo/synthetic-grok/chat_history.jsonl"
        )
        .as_slice(),
    ]
    .into_iter()
    .flat_map(|bytes| {
        bytes
            .split_inclusive(|byte| *byte == b'\n')
            .map(<[u8]>::to_vec)
    })
    .collect();
    let objects = serde_json::json!({"z":"first","a":false,"r":4,"b":"last", "items":(0..64).map(|i| serde_json::json!({"z":i,"a":false,"label":"fixture"})).collect::<Vec<_>>()});
    let tool = serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"fixture-call","output":serde_json::to_string(&objects).unwrap()}});
    vec![
        ("native_17", native, 1000),
        (
            "small_objects",
            vec![serde_json::to_vec(&objects).unwrap()],
            1000,
        ),
        (
            "text_64k",
            vec![serde_json::to_vec(&serde_json::json!({"text":"x".repeat(64 * 1024)})).unwrap()],
            300,
        ),
        (
            "tool_json_string",
            vec![serde_json::to_vec(&tool).unwrap()],
            1000,
        ),
    ]
}

#[test]
fn direct_value_matches_tree_serialization_and_identical_logical_charges() {
    let limits = Limits {
        inline_string_bytes: 2 * 1024 * 1024,
        resident_bytes: 8 * 1024 * 1024,
        depth: 128,
        ..Limits::default()
    };
    for (name, records, _) in cpu_cases() {
        for record in records {
            let serde: Value = serde_json::from_slice(&record).unwrap();
            for chunk in [1, 3, BUFFER] {
                let (tree, before) =
                    scan_into::<_, Node>(Chunked::new(record.as_slice(), chunk), limits).unwrap();
                let (value, after) =
                    scan_into::<_, Value>(Chunked::new(record.as_slice(), chunk), limits).unwrap();
                assert_eq!(before, after, "logical budgets changed for {name}/{chunk}");
                assert_eq!(
                    serde_json::to_vec(&value).unwrap(),
                    serde_json::to_vec(&tree.into_value().unwrap()).unwrap()
                );
                assert_eq!(
                    serde_json::to_vec(&value).unwrap(),
                    serde_json::to_vec(&serde).unwrap()
                );
                let exact = Limits {
                    resident_bytes: before.peak_resident_bytes,
                    ..limits
                };
                assert!(scan_value(Chunked::new(record.as_slice(), chunk), exact).is_ok());
                let below = Limits {
                    resident_bytes: before.peak_resident_bytes - 1,
                    ..limits
                };
                assert_eq!(
                    scan_value(Chunked::new(record.as_slice(), chunk), below)
                        .unwrap_err()
                        .kind,
                    ErrorKind::ResidentLimit
                );
            }
        }
    }
}

#[test]
fn direct_value_rejects_spans_and_preserves_grammar_error_checks() {
    let limits = Limits {
        inline_string_bytes: 2,
        ..Limits::default()
    };
    for bytes in [
        br#""long ordinary text""#.as_slice(),
        br#"{"x":"long ordinary text"}"#,
        br#"["long ordinary text"]"#,
    ] {
        assert_eq!(
            scan_value(bytes, limits).unwrap_err(),
            scan(bytes, limits).unwrap().into_value().unwrap_err()
        );
    }
    let representative = br#"{"z":[true,false,null,-0,1.25e-2],"a":{"\u0062":"\ud83d\ude00"}}"#;
    for end in 0..=representative.len() {
        for chunk in [1, 7, BUFFER] {
            let input = &representative[..end];
            let tree =
                scan(Chunked::new(input, chunk), Limits::default()).and_then(Document::into_value);
            let direct = scan_value(Chunked::new(input, chunk), Limits::default());
            match (tree, direct) {
                (Ok(a), Ok(b)) => assert_eq!(
                    serde_json::to_vec(&a).unwrap(),
                    serde_json::to_vec(&b).unwrap()
                ),
                (Err(a), Err(b)) => assert_eq!(a, b),
                _ => panic!("direct/tree grammar differs at prefix {end}, chunk {chunk}"),
            }
        }
    }
    let nested = format!("{}null{}", "[".repeat(HARD_DEPTH), "]".repeat(HARD_DEPTH));
    assert!(
        scan_value(
            nested.as_bytes(),
            Limits {
                depth: HARD_DEPTH,
                ..Limits::default()
            }
        )
        .is_ok()
    );
}

/// Explicit opt-in, in-memory parser CPU smoke; no service/native files/CLI.
/// Use a release test build, one test thread and an otherwise idle machine.
/// Timings include output destruction and are observations, never assertions.
#[test]
#[ignore = "explicit isolated parser CPU benchmark"]
fn scanner_cpu_benchmark() {
    use std::{hint::black_box, time::Instant};
    let limits = Limits {
        inline_string_bytes: 2 * 1024 * 1024,
        resident_bytes: 8 * 1024 * 1024,
        depth: 128,
        ..Limits::default()
    };
    for (name, records, repeats) in cpu_cases() {
        let mut samples = [Vec::new(), Vec::new()];
        for sample in 0..5 {
            for variant in 0..2 {
                // Alternate adjacent orders to reduce systematic warmup bias.
                let mode = (sample + variant) % 2;
                let start = Instant::now();
                for _ in 0..repeats {
                    for record in &records {
                        let value = if mode == 0 {
                            scan(black_box(record.as_slice()), limits)
                                .unwrap()
                                .into_value()
                                .unwrap()
                        } else {
                            scan_value(black_box(record.as_slice()), limits).unwrap()
                        };
                        black_box(value);
                    }
                }
                samples[mode].push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        samples
            .iter_mut()
            .for_each(|sample| sample.sort_by(f64::total_cmp));
        println!(
            "SCANNER_CPU case={name} records={} tree_value_ms={:?} direct_value_ms={:?} median_change_pct={:.2}",
            records.len() * repeats,
            samples[0],
            samples[1],
            (samples[1][2] / samples[0][2] - 1.0) * 100.0
        );
    }
}
