use super::*;
use crate::{
    media::{NativeImage, NativeSpan},
    native_replay::ReplayReader,
    sessions::records::native_images,
};
use serde_json::{Value, json};
use std::{
    io::Cursor,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn synthetic_root() -> PathBuf {
    std::env::temp_dir().join("sessiondock-tool-envelope-fixture")
}
fn synthetic_path() -> PathBuf {
    synthetic_root().join("session.jsonl")
}

type SourceReader = io::Take<Cursor<Arc<[u8]>>>;
struct Checked {
    reader: ReplayReader<SourceReader>,
    finishes: Arc<AtomicUsize>,
    fail_finish: bool,
}
impl Read for Checked {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.reader.read(out)
    }
}
impl CheckedReplay for Checked {
    fn finish(self: Box<Self>) -> Result<(), String> {
        self.finishes.fetch_add(1, Ordering::SeqCst);
        self.reader.finish().map_err(|error| error.to_string())?;
        if self.fail_finish {
            return Err("synthetic checked finish failed".into());
        }
        Ok(())
    }
}
fn opener(
    source: Arc<[u8]>,
    opens: Arc<AtomicUsize>,
    finishes: Arc<AtomicUsize>,
    fail_finish: bool,
) -> impl FnMut(&DecodePlan) -> Result<Box<dyn CheckedReplay>, String> {
    move |plan| {
        opens.fetch_add(1, Ordering::SeqCst);
        let first = plan.first();
        if first.end > source.len() as u64 {
            return Err("fixture range outside source".into());
        }
        let mut source = Cursor::new(source.clone());
        source.set_position(first.start);
        let reader = ReplayReader::new(source.take(first.end - first.start), plan)
            .map_err(|error| error.to_string())?;
        Ok(Box::new(Checked {
            reader,
            finishes: finishes.clone(),
            fail_finish,
        }))
    }
}
fn outer(text: &str) -> (Arc<[u8]>, TextSpan) {
    let source: Arc<[u8]> = serde_json::to_vec(text).unwrap().into();
    let document = scanner::scan(
        source.as_ref(),
        scanner::Limits {
            inline_string_bytes: 0,
        },
    )
    .unwrap();
    let Node::String(Text::Span(span)) = document.root else {
        panic!("expected span")
    };
    (source, span)
}
fn envelope(output: Value) -> Value {
    json!({"wall_time_seconds":0.25,"exit_code":3,"output":output})
}
fn image() -> Value {
    json!({"type":"input_image","image_url":format!("data:image/png;base64,{}", "A".repeat(2 * 1024 * 1024 + 4))})
}

type Prepared = (Value, Vec<native_images::Sidecar>, Arc<[u8]>);
fn prepare(output: Value) -> Result<Prepared, String> {
    let record = json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call","output":output}});
    let source: Arc<[u8]> = serde_json::to_vec(&record).unwrap().into();
    let document =
        scanner::scan(source.as_ref(), scanner::Limits::native(2 * 1024 * 1024)).unwrap();
    let opens = Arc::new(AtomicUsize::new(0));
    let finishes = Arc::new(AtomicUsize::new(0));
    let (value, sidecars) = native_images::prepare_with_replay(
        "codex",
        document.root,
        |image| {
            let (start, end) = image
                .plan
                .as_ref()
                .map_or((image.span.start(), image.span.end()), |plan| {
                    (plan.first().start, plan.first().end)
                });
            NativeImage::from_native_span(NativeSpan {
                root: synthetic_root(),
                path: synthetic_path(),
                file_identity: "fixture".into(),
                record_start: 0,
                record_end: source.len() as u64,
                start,
                end,
                plan: image.plan,
                decoded_len: image.span.decoded_len(),
                decoded_digest: *image.span.digest(),
                mime: image.mime,
                encoded_offset: image.encoded_offset,
                payload_digest: image.payload_digest,
            })
            .map_err(|error| error.to_string())
        },
        opener(source.clone(), opens.clone(), finishes.clone(), false),
        |span| {
            let plan = extend(None, span)?;
            let mut reader = opener(source.clone(), opens.clone(), finishes.clone(), false)(&plan)?;
            let mut text = String::new();
            reader
                .read_to_string(&mut text)
                .map_err(|_| "fixture text read")?;
            reader.finish()?;
            Ok(text)
        },
    )?;
    assert_eq!(
        opens.load(Ordering::SeqCst),
        finishes.load(Ordering::SeqCst)
    );
    Ok((value, sidecars, source))
}

#[test]
fn candidates_keep_last_output_then_root_then_object_lines() {
    let text = b"log\nOutput:\n{first}\nOutput:\n{last}";
    let positions = candidates(&mut text.as_slice()).unwrap();
    assert_eq!(positions[0].0, 28);
    assert_eq!(positions[1].0, 0);
    assert_eq!(positions[2].0, 12);
}

#[test]
fn output_marker_and_candidate_base_produce_replayable_final_image_plan() {
    let encoded = format!("a log before\nOutput:\n{}", envelope(json!([image()])));
    let (value, sidecars, source) = prepare(json!(encoded)).unwrap();
    assert_eq!(sidecars.len(), 1);
    assert!(value.to_string().len() < 512);
    let span = sidecars[0].image.as_ref().unwrap().native_span().unwrap();
    let plan = span.plan.as_ref().unwrap();
    assert_eq!(plan.ranges().len(), 2);
    let mut open = opener(
        source,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        false,
    );
    let mut reader = open(plan).unwrap();
    let mut decoded = String::new();
    reader.read_to_string(&mut decoded).unwrap();
    reader.finish().unwrap();
    assert!(decoded.starts_with("data:image/png;base64,"));
    assert_eq!(decoded.len() as u64, span.decoded_len);
    let (_, events, error) = crate::sessions::providers::parse_with_media(
        "codex",
        &synthetic_path(),
        &[(value, 1)],
        None,
        "fixture",
        &std::collections::BTreeMap::from([(1, sidecars)]),
    );
    assert_eq!(error, None);
    assert_eq!(
        events.iter().map(|event| event.media.len()).sum::<usize>(),
        1
    );
    let event = events.iter().find(|event| !event.media.is_empty()).unwrap();
    assert_eq!(event.message["exit_code"], 3);
    assert_eq!(event.message["duration_s"], 0.25);
    assert_eq!(event.message["call_id"], "call");
}

#[test]
fn mixed_structured_and_nested_string_envelopes_decode() {
    for layers in [1, 2, 8, 9] {
        let mut output = json!([image()]);
        for layer in 0..layers {
            let wrapper = envelope(output);
            output = if layer % 2 == 0 {
                json!(format!("prefix\nOutput:\n{wrapper}"))
            } else {
                wrapper
            };
        }
        let result = prepare(output);
        assert_eq!(result.unwrap().1.len(), 1, "{layers}");
    }
}

#[test]
fn eight_string_layers_plus_final_image_range_are_bounded_and_work() {
    let mut output = json!([image()]);
    for _ in 0..8 {
        output = json!(envelope(output).to_string());
    }
    let (_, images, _) = prepare(output).unwrap();
    assert_eq!(
        images[0]
            .image
            .as_ref()
            .unwrap()
            .native_span()
            .unwrap()
            .plan
            .as_ref()
            .unwrap()
            .ranges()
            .len(),
        9
    );
}

#[test]
fn single_text_wrappers_work_but_multiple_giant_pieces_or_unconsumed_fields_fail() {
    let text = envelope(json!([image()])).to_string();
    for output in [
        json!({"text":text}),
        json!({"type":"text","text":text}),
        json!([{"type":"text","text":text}]),
    ] {
        assert_eq!(prepare(output).unwrap().1.len(), 1);
    }
    // Every part of a multi-part output is its own reviewed
    // position, so the giant part decodes in place next to a small one; an
    // MCP content array with more than one giant block still fails.
    let (value, sidecars, _) =
        prepare(json!([{"type":"text","text":text},{"type":"text","text":"extra"}])).unwrap();
    assert_eq!(sidecars.len(), 1);
    assert_eq!(value["payload"]["output"][0]["exit_code"], 3);
    assert_eq!(value["payload"]["output"][1]["text"], "extra");
    assert_eq!(prepare(json!({"content":[{"type":"text","text":text},{"type":"text","text":text}],"isError":false})).unwrap().1.len(), 2);
    assert!(
        prepare(json!({"type":"text","text":text,"metadata":"x".repeat(2 * 1024 * 1024 + 1)}))
            .is_ok()
    );
}

#[test]
fn failed_candidates_always_finish_and_source_finish_failure_is_fatal() {
    let raw = format!("not json\n{}", envelope(json!({"ok":true})));
    let (source, span) = outer(&raw);
    let opens = Arc::new(AtomicUsize::new(0));
    let finishes = Arc::new(AtomicUsize::new(0));
    let result = parse(
        &span,
        None,
        &mut opener(source.clone(), opens.clone(), finishes.clone(), false),
    )
    .unwrap()
    .expect("envelope found");
    assert_eq!(result.candidate_start, 9);
    assert_eq!(opens.load(Ordering::SeqCst), 3);
    assert_eq!(finishes.load(Ordering::SeqCst), 3);
    let opens = Arc::new(AtomicUsize::new(0));
    let finishes = Arc::new(AtomicUsize::new(0));
    assert!(
        parse(
            &span,
            None,
            &mut opener(source, opens.clone(), finishes.clone(), true)
        )
        .is_err()
    );
    assert_eq!(opens.load(Ordering::SeqCst), 1);
    assert_eq!(finishes.load(Ordering::SeqCst), 1);
}

#[test]
fn all_candidates_are_read_and_duplicate_keys_use_the_last_value() {
    let raw = "{invalid}\n".repeat(40);
    let (source, span) = outer(&raw);
    let opens = Arc::new(AtomicUsize::new(0));
    let finishes = Arc::new(AtomicUsize::new(0));
    let result = parse(
        &span,
        None,
        &mut opener(source, opens.clone(), finishes.clone(), false),
    )
    .unwrap();
    assert!(result.is_none());
    assert_eq!(opens.load(Ordering::SeqCst), 41);
    assert_eq!(finishes.load(Ordering::SeqCst), 41);
    let (source, span) =
        outer(r#"{"output":[],"wall_time_seconds":1,"exit_code":0,"\u0065xit_code":2}"#);
    let parsed = parse(
        &span,
        None,
        &mut opener(
            source,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            false,
        ),
    )
    .unwrap()
    .unwrap();
    assert_eq!(parsed.root.into_value().unwrap()["exit_code"], json!(2));
}

#[test]
fn ordinary_tutorial_and_arguments_are_not_replay_sources() {
    let data = envelope(json!([image()])).to_string();
    let (value, sidecars, _) = prepare(json!({"business":data})).unwrap();
    assert!(sidecars.is_empty());
    assert_eq!(value["payload"]["output"]["business"], data);
    let record = json!({"type":"response_item","payload":{"type":"function_call","name":"tool","arguments":data}});
    let bytes = serde_json::to_vec(&record).unwrap();
    let root = scanner::scan(bytes.as_slice(), scanner::Limits::default())
        .unwrap()
        .root;
    assert!(
        native_images::prepare_with_replay(
            "codex",
            root,
            |_| panic!("not an image"),
            |_| panic!("not authorized to replay"),
            |_| Err("原生文本读回来源未配置".into())
        )
        .is_err()
    );
}

#[test]
fn small_tool_strings_keep_legacy_metadata_text_and_candidate_priority_exactly() {
    let first = envelope(json!("first"));
    let last = json!({"chunk_id":"chosen","output":"last","exit_code":7,"wall_time_seconds":1.5});
    let cases = [
        json!(first.to_string()),
        json!(format!("log\nOutput:\n{first}\nOutput:\n{last}")),
        json!({"text":format!("log\n{first}")}),
        json!([{"type":"text","text":first.to_string()}]),
        json!([{"type":"text","text":"prefix"},{"type":"text","text":last.to_string()}]),
        json!("普通 JSON 教程 {\"image_url\":\"https://example.invalid/a.png\"}"),
        json!({"content":[{"metric":2}]}),
        json!(format!("  \n{last}\t\n")),
        json!({"z":"first","a":false,"r":4,"b":"last"}),
    ];
    for output in cases {
        let record = json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":output}});
        let bytes = serde_json::to_vec(&record).unwrap();
        let root = scanner::scan(bytes.as_slice(), scanner::Limits::default())
            .unwrap()
            .root;
        let (prepared, sidecars) = native_images::prepare_with_replay(
            "codex",
            root,
            |_| panic!("small fixture has no image span"),
            |_| panic!("small fixture must not reopen source"),
            |_| panic!("small fixture has no text span"),
        )
        .unwrap();
        assert!(sidecars.is_empty());
        assert_eq!(serde_json::to_vec(&prepared).unwrap(), bytes);
        let parse = |record| {
            crate::sessions::providers::parse(
                "codex",
                &synthetic_path(),
                &[(record, 1)],
                None,
                "fixture",
            )
        };
        let (old_meta, old_events, old_error) = parse(record);
        let (new_meta, new_events, new_error) = parse(prepared);
        assert_eq!(old_meta, new_meta);
        assert_eq!(old_error, new_error);
        assert_eq!(
            old_events
                .iter()
                .map(|event| &event.message)
                .collect::<Vec<_>>(),
            new_events
                .iter()
                .map(|event| &event.message)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn legacy_mcp_string_envelope_oracle_preserves_outer_error_and_inner_metadata() {
    for error in [false, true] {
        let inner = json!({"output":"inner text","exit_code":0,"wall_time_seconds":0.5});
        let record = json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"mcp","output":{"content":[{"type":"text","text":inner.to_string()}],"isError":error}}});
        let (_, events, unsupported) = crate::sessions::providers::parse(
            "codex",
            &synthetic_path(),
            &[(record, 1)],
            None,
            "fixture",
        );
        assert_eq!(unsupported, None);
        let event = events
            .iter()
            .find(|event| event.message["call_id"] == "mcp")
            .unwrap();
        assert_eq!(event.message["error"], error);
        assert_eq!(event.message["exit_code"], 0);
        assert_eq!(event.message["duration_s"], 0.5);
        assert_eq!(event.message["text"], "inner text");
    }
}

#[test]
fn mcp_giant_single_source_matches_small_oracle_without_public_markers() {
    fn messages(output: Value, prepared: bool) -> Vec<Value> {
        let record = json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call","output":output}});
        let result = if prepared {
            let (value, sidecars, _) = prepare(record["payload"]["output"].clone()).unwrap();
            assert!(serde_json::to_vec(&value).unwrap().len() < 2048);
            crate::sessions::providers::parse_with_media(
                "codex",
                &synthetic_path(),
                &[(value, 1)],
                None,
                "fixture",
                &std::collections::BTreeMap::from([(1, sidecars)]),
            )
        } else {
            crate::sessions::providers::parse(
                "codex",
                &synthetic_path(),
                &[(record, 1)],
                None,
                "fixture",
            )
        };
        assert_eq!(result.2, None);
        result.1.into_iter().map(|event| event.message).collect()
    }
    let small_image = json!({"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="});
    for images in [false, true] {
        for error in [None, Some(false), Some(true)] {
            for location in [0, 1, 2] {
                let build = |large| {
                    let content = if images {
                        json!([if large { image() } else { small_image.clone() }])
                    } else {
                        json!("inner text")
                    };
                    let inner = json!({"output":content,"exit_code":0,"wall_time_seconds":0.5});
                    let prefix = if large && !images {
                        "x".repeat(2 * 1024 * 1024 + 1)
                    } else {
                        "log".into()
                    };
                    let mut mcp = json!({"content":[{"type":"text","text":format!("{prefix}\nOutput:\n{inner}")}]});
                    if let Some(error) = error {
                        mcp["isError"] = json!(error);
                    }
                    match location {
                        0 => mcp,
                        1 => json!({"output":mcp,"exit_code":0,"wall_time_seconds":1.5}),
                        _ => json!([mcp]),
                    }
                };
                let old = messages(build(false), false);
                let new = messages(build(true), true);
                assert_eq!(
                    serde_json::to_vec(&new).unwrap(),
                    serde_json::to_vec(&old).unwrap(),
                    "images={images} error={error:?} location={location}"
                );
            }
        }
    }
}

#[test]
fn known_envelope_wins_over_mcp_shaped_extra_fields_without_dropping_unknown_span() {
    let value = json!({"output":"chosen","exit_code":0,"wall_time_seconds":1,"content":[{"type":"text","text":"x".repeat(100)}],"isError":true});
    let bytes = serde_json::to_vec(&value).unwrap();
    let root = scanner::scan(
        bytes.as_slice(),
        scanner::Limits {
            inline_string_bytes: 64,
        },
    )
    .unwrap()
    .root;
    assert!(known(&root));
    assert!(candidate(&root).unwrap().is_none());
    assert!(root.into_value().is_err());
}

#[test]
fn unicode_candidate_trim_matches_small_oracle_and_replays_exact_image_offsets() {
    let small = json!({"output":"inner","exit_code":0,"wall_time_seconds":0.5});
    let raw = format!("\u{2003}{small}\u{00a0}");
    let record = json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":raw}});
    let (_, old, error) = crate::sessions::providers::parse(
        "codex",
        &synthetic_path(),
        &[(record, 1)],
        None,
        "fixture",
    );
    assert_eq!(error, None);
    assert!(old.iter().any(|event| event.message["text"] == "inner"));
    let (source, span) = outer(&raw);
    let parsed = parse(
        &span,
        None,
        &mut opener(
            source,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            false,
        ),
    )
    .unwrap()
    .expect("envelope found");
    assert_eq!(parsed.candidate_start, 3);
    assert_eq!(parsed.root.into_value().unwrap(), small);
    for prefix in ["\u{2003}", "log\n\u{2003}", "log\nOutput:\n\u{2003}"] {
        let text = format!("{prefix}{}\u{a0}\u{3000}", envelope(json!([image()])));
        let (_, images, source) = prepare(json!(text)).unwrap();
        let span = images[0].image.as_ref().unwrap().native_span().unwrap();
        let mut reader = opener(
            source,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            false,
        )(span.plan.as_ref().unwrap())
        .unwrap();
        let mut data = String::new();
        reader.read_to_string(&mut data).unwrap();
        reader.finish().unwrap();
        assert!(data.starts_with("data:image/png;base64,"));
        assert_eq!(data.len() as u64, span.decoded_len);
    }
}

#[test]
fn unicode_discovery_is_chunk_bounded_and_does_not_rewrite_json_interior() {
    struct Chunked<'a> {
        data: &'a [u8],
        chunk: usize,
    }
    impl Read for Chunked<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let amount = out.len().min(self.chunk);
            self.data.read(&mut out[..amount])
        }
    }
    let raw = "\u{2003}log\nOutput:\n\u{a0}\u{3000}{\"output\":\"inside \u{2003} text\",\"wall_time_seconds\":1,\"exit_code\":0}\u{a0}";
    let start = raw.find('{').unwrap() as u64;
    let end = raw.rfind('}').unwrap() as u64 + 1;
    for chunk in [1, 2, 3, 4, 7, 8192] {
        let result = candidates(&mut Chunked {
            data: raw.as_bytes(),
            chunk,
        })
        .unwrap();
        assert_eq!(result[0], (start, end));
    }
    for data in [b"\xf0\x80\x80\x80".as_slice(), b"\xe2\x80".as_slice()] {
        assert!(candidates(&mut Chunked { data, chunk: 1 }).is_err());
    }
    let (source, span) = outer("{\u{2003}\"output\":[],\"wall_time_seconds\":1,\"exit_code\":0}");
    // Unicode whitespace inside the JSON is not trimmed away: no envelope
    // is found, and the string stays ordinary giant text.
    assert!(
        parse(
            &span,
            None,
            &mut opener(
                source,
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
                false
            )
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn nested_giant_ordinary_text_is_replayed_without_becoming_media() {
    let text = format!("{}中文", "x".repeat(2 * 1024 * 1024 + 1));
    let (value, sidecars, _) = prepare(json!(
        envelope(json!({"content":[{"type":"text","text":text}]})).to_string()
    ))
    .unwrap();
    assert!(sidecars.is_empty());
    assert_eq!(
        value["payload"]["output"]["output"]["content"][0]["text"],
        text
    );
}

#[test]
fn large_envelope_ast_materializes() {
    // A giant ordinary field selects streaming replay even with many sibling nodes.
    let text = "x".repeat(2 * 1024 * 1024 + 1);
    let output = json!({"text":text, "items":vec![Value::Null; 50_000]});
    let (value, sidecars, _) = prepare(json!(envelope(output.clone()).to_string())).unwrap();
    assert!(sidecars.is_empty());
    assert_eq!(value["payload"]["output"]["output"], output);
}

#[test]
fn streamed_envelope_after_more_than_thirty_two_candidates_is_found() {
    let raw = format!("{}{}", "{invalid}\n".repeat(40), envelope(json!("FOUND")));
    let (source, span) = outer(&raw);
    let found = parse(
        &span,
        None,
        &mut opener(
            source,
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            false,
        ),
    )
    .unwrap()
    .unwrap();
    assert_eq!(found.root.into_value().unwrap()["output"], "FOUND");
}
