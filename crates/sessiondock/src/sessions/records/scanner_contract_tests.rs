//! Independent differential contract against serde_json, using only synthetic
//! repository fixtures and deterministic in-memory input. No provider execution,
//! native-home discovery, Python service, or image decoding is involved.

use super::scanner::{ErrorKind, Limits, scan};
use serde_json::{Map, Value, json};
use std::io::{self, Read};

const CHUNKS: [usize; 4] = [1, 2, 7, 64 * 1024];

fn strict_limits() -> Limits {
    Limits {
        inline_string_bytes: 2 * 1024 * 1024,
        resident_bytes: 8 * 1024 * 1024,
        depth: 128,
        ..Limits::default()
    }
}

struct Chunked<'a> {
    remaining: &'a [u8],
    chunk: usize,
}

impl Read for Chunked<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = output.len().min(self.chunk).min(self.remaining.len());
        output[..count].copy_from_slice(&self.remaining[..count]);
        self.remaining = &self.remaining[count..];
        Ok(count)
    }
}

fn assert_exact(input: &[u8], label: &str) {
    let expected = serde_json::from_slice::<Value>(input)
        .unwrap_or_else(|error| panic!("synthetic baseline {label}: {error}"));
    for chunk in CHUNKS {
        let document = scan(
            Chunked {
                remaining: input,
                chunk,
            },
            strict_limits(),
        )
        .unwrap_or_else(|error| panic!("scanner rejected {label}, chunk={chunk}: {error:?}"));
        let actual = document.into_value().unwrap_or_else(|error| {
            panic!("strict inline record was not materialized: {label}, chunk={chunk}: {error:?}")
        });
        assert!(
            actual == expected,
            "scanner changed Value: {label}, chunk={chunk}"
        );
        // Value equality ignores object insertion order even when serde_json
        // uses preserve_order. Tool argument presentation consumes that order.
        assert!(
            serde_json::to_vec(&actual).unwrap() == serde_json::to_vec(&expected).unwrap(),
            "scanner changed serialized object order: {label}, chunk={chunk}"
        );
    }
}

#[test]
fn tool_parameter_and_nested_object_insertion_order_is_preserved() {
    // Deliberately not json! or a sorted map: the byte spelling establishes
    // source order at every level, including opaque JSON-string arguments.
    for (index, bytes) in [
        br#"{"z":0,"a":1,"middle":{"last":2,"first":3}}"#.as_slice(),
        br#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"synthetic_tool","input":{"z_path":"fixture","a_command":"never executed","options":{"zeta":true,"alpha":false}},"id":"synthetic-call"}]}}"#,
        br#"{"type":"response_item","payload":{"type":"function_call","name":"synthetic_tool","arguments":"{\"z\":1,\"a\":2}","call_id":"synthetic-call","metadata":{"z":1,"a":2}}}"#,
        br#"{"type":"assistant","tool_calls":[{"name":"synthetic_tool","arguments":{"z_command":"never executed","a_path":"fixture","nested":[{"y":1,"b":2},{"c":3,"a":4}]},"id":"synthetic-call"}],"content":""}"#,
    ]
    .into_iter()
    .enumerate()
    {
        assert_exact(bytes, &format!("unsorted native/tool object {index}"));
    }
}

fn assert_both_reject(input: &[u8], label: &str) {
    assert!(
        serde_json::from_slice::<Value>(input).is_err(),
        "synthetic invalid baseline accepted {label}"
    );
    for chunk in CHUNKS {
        assert!(
            scan(
                Chunked {
                    remaining: input,
                    chunk,
                },
                strict_limits(),
            )
            .is_err(),
            "scanner accepted malformed {label}, chunk={chunk}"
        );
    }
}

#[test]
fn repository_provider_jsonl_complete_lines_match_serde_exactly() {
    let fixtures: [(&str, &[u8]); 3] = [
        (
            "claude",
            include_bytes!("../../../tests/fixtures/claude/project-demo/synthetic-claude.jsonl"),
        ),
        (
            "codex",
            include_bytes!(
                "../../../tests/fixtures/codex/2026/09/11/rollout-synthetic-codex.jsonl"
            ),
        ),
        (
            "grok",
            include_bytes!(
                "../../../tests/fixtures/grok/project-demo/synthetic-grok/chat_history.jsonl"
            ),
        ),
    ];
    let mut lines = 0;
    for (provider, bytes) in fixtures {
        assert_eq!(
            bytes.last(),
            Some(&b'\n'),
            "fixture must be fully committed"
        );
        for (index, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
            assert_exact(line, &format!("{provider} complete line {index}"));
            lines += 1;
        }
    }
    assert_eq!(lines, 17);
}

#[test]
fn native_blocks_and_nested_tool_envelopes_remain_exact_json_values() {
    // These are parser fixtures, not media-decoder or executable tool inputs.
    let image = json!({"type":"image","source":{"type":"base64",
        "media_type":"image/png","data":"synthetic-parser-payload"}});
    let mcp = json!({"isError":false,"content":[{"type":"text","text":"图片 before 后"},
        {"type":"image","mimeType":"image/png","data":"synthetic-parser-payload"}],
        "structuredContent":{"missing":null,"numbers":[-0.0,1.25,9007199254740993u64]}});
    let cases = [
        json!({"type":"user","uuid":"synthetic-user","parentUuid":null,
            "message":{"role":"user","content":[{"type":"text","text":"正文"},image]}}),
        json!({"type":"assistant","message":{"role":"assistant","content":[
            {"type":"thinking","thinking":"人工思考","signature":"opaque-unchanged"},
            {"type":"tool_use","id":"synthetic-call","name":"mcp__fixture__image",
             "input":{"literal":"{\"data\":\"ordinary JSON string\"}","empty":{}}}]}}),
        json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"synthetic-call","is_error":false,
             "content":[{"type":"text","text":"result"},image]}]}}),
        json!({"type":"response_item","payload":{"type":"function_call",
            "name":"mcp__fixture__image","call_id":"synthetic-call","turn_id":"turn-1",
            "arguments":serde_json::to_string(&json!({"text":"quote\" slash\\ nul\0 中文😀"})).unwrap()}}),
        json!({"type":"response_item","payload":{"type":"function_call_output",
            "call_id":"synthetic-call","output":mcp}}),
        json!({"type":"response_item","payload":{"type":"function_call_output",
            "call_id":"synthetic-call","output":serde_json::to_string(&mcp).unwrap()}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant",
            "phase":"commentary","turn_id":"turn-1","content":[
                {"type":"output_text","text":"literal [图片]"},
                {"type":"input_image","image_url":"data:image/png;base64,synthetic-parser-payload"}]}}),
        json!({"type":"tool_result","tool_call_id":"synthetic-call","content":mcp,
            "responseMetadata":{"latency":0,"optional":null}}),
        json!({"type":"user","prompt_index":7,"content":[
            {"type":"image_url","image_url":{"url":"data:image/png;base64,synthetic-parser-payload"}},
            {"type":"text","text":"工具 JSON 不被解包：{\"content\":[]}"}]}),
    ];
    for (index, value) in cases.into_iter().enumerate() {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        assert_exact(&bytes, &format!("native/tool envelope {index}"));
    }
}

struct Seed(u64);

impl Seed {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn value(&mut self, depth: usize) -> Value {
        let choice = self.next() % if depth == 0 { 6 } else { 8 };
        match choice {
            0 => Value::Null,
            1 => Value::Bool(self.next() & 1 != 0),
            2 => json!(self.next()),
            3 => json!((self.next() % 2_000_000) as i64 - 1_000_000),
            4 => json!((self.next() % 100_000) as f64 / 32.0),
            5 => {
                let samples = [
                    "",
                    "plain",
                    "中文😀",
                    "quote\"\\/\0\n\r\t",
                    "é\u{2028}\u{2029}",
                ];
                let text = samples[(self.next() as usize) % samples.len()];
                Value::String(format!("{text}{}", self.next() % 97))
            }
            6 => {
                let length = (self.next() % 6) as usize;
                Value::Array((0..length).map(|_| self.value(depth - 1)).collect())
            }
            _ => {
                let length = (self.next() % 6) as usize;
                let mut fields = Map::new();
                for index in 0..length {
                    let key = format!("key-{:016x}-{index}-中文\\\"", self.next());
                    fields.insert(key, self.value(depth - 1));
                }
                Value::Object(fields)
            }
        }
    }
}

#[test]
fn seeded_values_match_serde_without_normalizing_fields_or_number_types() {
    let mut seed = Seed(0x6c61_7a79_7363_616e);
    for index in 0..512 {
        let value = seed.value(4);
        for (encoding, bytes) in [
            ("compact", serde_json::to_vec(&value).unwrap()),
            ("pretty", serde_json::to_vec_pretty(&value).unwrap()),
        ] {
            assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), value);
            assert_exact(&bytes, &format!("seeded {index} {encoding}"));
        }
    }
}

#[test]
fn number_boundaries_and_valid_unicode_escape_spellings_match_serde() {
    for number in [
        "0",
        "-0",
        "0.0",
        "-0.0",
        "1",
        "-1",
        "1e0",
        "1E+2",
        "1e-2",
        "0.125",
        "9007199254740993",
        "18446744073709551615",
        "18446744073709551616",
        "-9223372036854775808",
        "-9223372036854775809",
        "1.7976931348623157e308",
        "2.2250738585072014e-308",
        "5e-324",
        "1e-400",
        "-1e-400",
    ] {
        assert_exact(number.as_bytes(), &format!("number {number}"));
    }
    for (index, bytes) in [
        br#""\u0000\b\f\n\r\t\/\\\"""#.as_slice(),
        br#""\uD83D\uDE00\ud83d\ude00\u0061""#.as_slice(),
        br#"{"\u0061":1,"\u4e2d\u6587":"\u4e2d\u6587","empty":[],"nil":null}"#.as_slice(),
        "\t\r\n {\"utf8\":\"中文😀é\u{2028}\u{2029}\",\"literal\":\"\\u0061\"} \r\n".as_bytes(),
        b" [ true , false , null , {} , [] ] \n".as_slice(),
    ]
    .into_iter()
    .enumerate()
    {
        assert_exact(bytes, &format!("valid escape/whitespace {index}"));
    }
}

#[test]
fn malformed_grammar_and_invalid_utf8_are_not_silently_accepted() {
    for (index, bytes) in [
        b"".as_slice(),
        b" \n\r\t",
        b"[1,]",
        b"{\"a\":1,}",
        b"{a:1}",
        b"{'a':1}",
        b"{\"a\" 1}",
        b"{\"a\":}",
        b"[1 2]",
        b"true false",
        b"{}{}",
        b"nullx",
        b"[}",
        b"[",
        b"{",
        b"\"unterminated",
        b"\"raw\nnewline\"",
        b"\"\x00\"",
        b"+1",
        b"01",
        b"-01",
        b".1",
        b"1.",
        b"1e",
        b"1e+",
        b"--1",
        b"NaN",
        b"Infinity",
        b"1e309",
        b"/* comment */null",
        b"null\x0b",
        b"null\x0c",
        br#""\x41""#,
        br#""\u12""#,
        br#""\uXXXX""#,
        br#""\uD800""#,
        br#""\uDC00""#,
        br#""\uD800\u0041""#,
        br#""\uDC00\uD800""#,
        b"\"\xc0\xaf\"",
        b"\"\xed\xa0\x80\"",
        b"\"\xf4\x90\x80\x80\"",
        b"\"\xf0\x9f\"",
        b"\"\x80\"",
        b"\xef\xbb\xbf{}",
    ]
    .into_iter()
    .enumerate()
    {
        assert_both_reject(bytes, &format!("bad grammar/utf8 {index}"));
    }
}

#[test]
fn every_truncated_prefix_matches_serde_acceptance_and_value() {
    let examples: [&[u8]; 4] = [
        br#"{"type":"tool_result","content":[{"text":"a\"b\\c\uD83D\uDE00"}],"ok":true}"#,
        br#"[null,false,true,-12.5e+2,{"empty":[]}]"#,
        "\"中文😀é\"".as_bytes(),
        b"-123.456e-12 \r\n",
    ];
    for (example, bytes) in examples.into_iter().enumerate() {
        for length in 0..=bytes.len() {
            let prefix = &bytes[..length];
            let expected = serde_json::from_slice::<Value>(prefix);
            for chunk in CHUNKS {
                let actual = scan(
                    Chunked {
                        remaining: prefix,
                        chunk,
                    },
                    strict_limits(),
                )
                .and_then(|document| document.into_value());
                match (&expected, actual) {
                    (Ok(expected), Ok(actual)) => {
                        assert!(
                            actual == *expected,
                            "prefix changed Value: example={example}, length={length}, chunk={chunk}"
                        );
                        assert!(
                            serde_json::to_vec(&actual).unwrap()
                                == serde_json::to_vec(expected).unwrap(),
                            "prefix changed object order: example={example}, length={length}, chunk={chunk}"
                        );
                    }
                    (Err(_), Err(_)) => {}
                    _ => panic!(
                        "prefix acceptance changed: example={example}, length={length}, chunk={chunk}"
                    ),
                }
            }
        }
    }
}

#[test]
fn nested_containers_match_serde_within_the_shared_depth_budget() {
    for depth in [0, 1, 2, 16, 63, 64, 100] {
        let array = format!("{}null{}", "[".repeat(depth), "]".repeat(depth));
        assert_exact(array.as_bytes(), &format!("array depth {depth}"));
        let object = format!("{}true{}", "{\"k\":".repeat(depth), "}".repeat(depth));
        assert_exact(object.as_bytes(), &format!("object depth {depth}"));
    }
    let too_deep = format!("{}null{}", "[".repeat(129), "]".repeat(129));
    assert_both_reject(too_deep.as_bytes(), "beyond both depth budgets");
}

#[test]
fn duplicate_keys_are_an_explicit_fail_closed_difference_from_serde() {
    for bytes in [
        br#"{"a":1,"a":2}"#.as_slice(),
        br#"{"a":1,"\u0061":2}"#,
        br#"{"nested":[{"x":null,"x":false}]}"#,
        br#"{"":1,"":1}"#,
    ] {
        assert!(serde_json::from_slice::<Value>(bytes).is_ok());
        for chunk in CHUNKS {
            let error = scan(
                Chunked {
                    remaining: bytes,
                    chunk,
                },
                strict_limits(),
            )
            .err()
            .expect("duplicate key must fail closed");
            assert_eq!(error.kind, ErrorKind::DuplicateKey);
        }
    }
    // Key identity is scoped to one object, not globally to the whole document.
    assert_exact(
        br#"{"left":{"a":1},"right":{"a":2}}"#,
        "same key in separate objects",
    );
}

#[test]
fn strict_two_mib_threshold_keeps_one_mib_ordinary_text_fully_inline() {
    let ordinary = "正文😀\\\"\n".repeat(100_000);
    assert!(ordinary.len() >= 1024 * 1024 && ordinary.len() < 2 * 1024 * 1024);
    let bytes = serde_json::to_vec(&json!({"type":"user","message":{"role":"user",
        "content":ordinary}}))
    .unwrap();
    assert_exact(
        &bytes,
        "ordinary >1MiB UTF8 string under strict 2MiB inline threshold",
    );
    let ascii = serde_json::to_vec(&json!({"body":"x".repeat(1024 * 1024)})).unwrap();
    assert_exact(&ascii, "ordinary exactly 1MiB ASCII string");
}

#[test]
fn default_span_cannot_be_silently_materialized_or_confused_with_strict_inline_mode() {
    let bytes = serde_json::to_vec(&json!({"body":"x".repeat(100_000)})).unwrap();
    let document = scan(bytes.as_slice(), Limits::default()).unwrap();
    assert_eq!(
        document.into_value().unwrap_err().kind,
        ErrorKind::UnmaterializedSpan
    );
    assert_exact(
        &bytes,
        "the same ordinary string is materialized in strict mode",
    );
}
