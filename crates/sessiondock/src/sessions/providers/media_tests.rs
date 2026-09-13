//! Synthetic embedded image records only; no paths, network or CLI are opened.
use super::*;

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

fn image(source: &str) -> Value {
    if source == "claude" {
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":PNG}})
    } else {
        json!({"type":"image_url","image_url":{"url":format!("data:image/png;base64,{PNG}")}})
    }
}

fn message(source: &str, role: &str, content: Value) -> Value {
    match source {
        "claude" => {
            json!({"type":role,"uuid":"synthetic-turn","message":{"content":content,"stop_reason":"end_turn"}})
        }
        "codex" => {
            json!({"type":"response_item","payload":{"type":"message","role":role,"content":content,"turn_id":"synthetic-turn","phase":"final_answer"}})
        }
        _ => json!({"type":role,"content":content,"prompt_index":7}),
    }
}

fn parse_records(source: &str, records: Vec<Value>) -> (Value, Vec<Event>, Option<String>) {
    let records = records
        .into_iter()
        .enumerate()
        .map(|(i, value)| (value, (i as u64 + 1) * 100))
        .collect::<Vec<_>>();
    parse(
        source,
        Path::new("synthetic.jsonl"),
        &records,
        None,
        "2026-09-12T00:00:00.000Z",
    )
}

fn assert_private(meta: &Value, events: &[Event]) {
    assert!(!meta.to_string().contains(PNG));
    for event in events {
        assert!(!event.message.to_string().contains(PNG));
        assert!(!event.message.to_string().contains("data:image"));
        assert!(event.message.get("media").is_none());
    }
}

#[test]
fn all_sources_keep_image_only_inputs_typed_and_assign_their_normal_turn() {
    for source in ["claude", "codex", "grok"] {
        let (meta, events, error) = parse_records(
            source,
            vec![message(source, "user", json!([image(source)]))],
        );
        assert!(error.is_none(), "{source}: {error:?}");
        let inputs = events
            .iter()
            .filter(|event| event.message["role"] == "user")
            .collect::<Vec<_>>();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].message["text"], "[图片]");
        assert_eq!(inputs[0].media.len(), 1);
        assert!(inputs[0].media[0].encoded_len() >= PNG.len());
        assert_eq!(inputs[0].end, 100);
        assert_eq!(
            inputs[0].message["turn_id"],
            if source == "grok" {
                "prompt:7"
            } else {
                "synthetic-turn"
            }
        );
        if source == "claude" {
            assert_eq!(events[0].message["state"], "working");
        }
        assert_private(&meta, &events);
    }
}

#[test]
fn several_image_only_blocks_use_native_counts_and_single_fallback_per_message() {
    for source in ["claude", "codex", "grok"] {
        for tool in [false, true] {
            let content = json!([image(source), image(source), image(source)]);
            let records = if tool {
                tool_records(source, content)
            } else {
                vec![message(source, "user", content)]
            };
            let (meta, events, error) = parse_records(source, records);
            assert!(error.is_none(), "{source}: {error:?}");
            let images = events
                .iter()
                .filter(|event| !event.media.is_empty())
                .collect::<Vec<_>>();
            assert_eq!(
                images.len(),
                if source == "claude" && !tool { 3 } else { 1 }
            );
            assert!(images.iter().all(|event| event.message["text"] == "[图片]"));
            assert_eq!(
                images.iter().map(|event| event.media.len()).sum::<usize>(),
                3
            );
            assert_private(&meta, &events);
        }
    }
}

#[test]
fn literal_image_placeholder_text_is_preserved_next_to_typed_images() {
    for source in ["claude", "codex", "grok"] {
        let (meta, events, error) = parse_records(
            source,
            tool_records(
                source,
                json!([{"type":"text","text":"literal [图片] remains"},image(source)]),
            ),
        );
        assert!(error.is_none(), "{source}: {error:?}");
        let event = events.last().unwrap();
        assert_eq!(event.message["text"], "literal [图片] remains");
        assert_eq!(event.media.len(), 1);
        assert_private(&meta, &events);
    }
}

#[test]
fn mixed_text_and_images_follow_native_message_counts_without_invented_placeholder_lines() {
    for source in ["claude", "codex", "grok"] {
        let text = |text: &str| json!({"type":"text","text":text});
        let content = json!([
            text("before"),
            image(source),
            text("between"),
            image(source),
            text("after")
        ]);
        let (meta, events, error) =
            parse_records(source, vec![message(source, "assistant", content)]);
        assert!(error.is_none(), "{source}: {error:?}");
        if source == "claude" {
            assert_eq!(events.len(), 5);
            assert_eq!(
                events
                    .iter()
                    .map(|event| event.message["text"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["before", "[图片]", "between", "[图片]", "after"]
            );
            assert_eq!(
                events
                    .iter()
                    .map(|event| event.media.len())
                    .collect::<Vec<_>>(),
                [0, 1, 0, 1, 0]
            );
        } else {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].message["text"], "before\nbetween\nafter");
            assert_eq!(events[0].media.len(), 2);
        }
        assert!(events.iter().all(|event| event.message["phase"] == "final"));
        assert!(events.iter().all(|event| event.end == 100));
        assert_private(&meta, &events);
        let (_, text_events, error) = parse_records(
            source,
            vec![message(
                source,
                "assistant",
                json!([text("before"), text("after")]),
            )],
        );
        assert!(error.is_none());
        if source == "claude" {
            assert_eq!(text_events.len(), 2);
            assert_eq!(text_events[0].message["text"], "before");
            assert_eq!(text_events[1].message["text"], "after");
        } else {
            assert_eq!(text_events.len(), 1);
            assert_eq!(text_events[0].message["text"], "before\nafter");
        }
        assert!(text_events.iter().all(|event| event.media.is_empty()));
    }
}

fn tool_records(source: &str, output: Value) -> Vec<Value> {
    match source {
        "claude" => vec![
            message(
                source,
                "assistant",
                json!([{"type":"tool_use","id":"synthetic-call","name":"capture","input":{}}]),
            ),
            message(
                source,
                "user",
                json!([{"type":"tool_result","tool_use_id":"synthetic-call","content":output}]),
            ),
        ],
        "codex" => vec![
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"synthetic-call","name":"capture","arguments":"{}","turn_id":"synthetic-turn"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"synthetic-call","output":output,"turn_id":"synthetic-turn"}}),
        ],
        _ => vec![
            json!({"type":"assistant","content":"","tool_calls":[{"id":"synthetic-call","name":"capture","arguments":{}}]}),
            json!({"type":"tool_result","tool_call_id":"synthetic-call","content":output}),
        ],
    }
}

#[test]
fn nested_tool_result_images_keep_call_identity_and_never_become_raw_output_json() {
    for source in ["claude", "codex", "grok"] {
        let output =
            json!([{"type":"text","text":"before"},[image(source)],{"type":"text","text":"after"}]);
        let (meta, events, error) = parse_records(source, tool_records(source, output));
        assert!(error.is_none(), "{source}: {error:?}");
        let result = events
            .iter()
            .find(|event| event.message["role"] == "tool_result")
            .unwrap();
        assert_eq!(result.message["text"], "before\nafter");
        assert_eq!(result.message["call_id"], "synthetic-call");
        assert_eq!(result.message["name"], "capture");
        assert_eq!(result.message["error"], false);
        assert_eq!(result.media.len(), 1);
        assert_eq!(result.end, 200);
        assert_private(&meta, &events);
    }
}

#[test]
fn codex_json_tool_envelopes_preserve_exit_and_duration_after_media_extraction() {
    let envelope = json!({"output":[{"type":"text","text":"capture"},image("codex")],"exit_code":2,"wall_time_seconds":0.25});
    for output in [
        envelope.clone(),
        json!(envelope.to_string()),
        json!([{"type":"text","text":format!("Script completed\nOutput:\n{envelope}")}]),
    ] {
        let (meta, events, error) = parse_records("codex", tool_records("codex", output));
        assert!(error.is_none(), "{error:?}");
        let event = events.last().unwrap();
        assert_eq!(event.message["role"], "tool_result");
        assert_eq!(event.message["text"], "capture");
        assert_eq!(event.message["exit_code"], 2);
        assert_eq!(event.message["duration_s"], 0.25);
        assert_eq!(event.message["error"], true);
        assert_eq!(event.message["turn_id"], "synthetic-turn");
        assert_eq!(event.media.len(), 1);
        assert_private(&meta, &events);
    }
}

#[test]
fn up_to_256_images_are_typed_per_event_and_257_fail_the_whole_event() {
    for source in ["claude", "codex", "grok"] {
        for count in [16, 17, 256, 257] {
            for tool in [false, true] {
                let images = Value::Array((0..count).map(|_| image(source)).collect());
                let records = if tool {
                    tool_records(source, images)
                } else {
                    vec![message(source, "user", images)]
                };
                let (meta, events, error) = parse_records(source, records);
                if count <= 256 {
                    assert!(error.is_none(), "{source} count={count}: {error:?}");
                    assert_eq!(
                        events.iter().map(|event| event.media.len()).sum::<usize>(),
                        count
                    );
                } else {
                    let error = error.unwrap();
                    assert!(error.contains("256"), "{source}: {error}");
                    assert!(!error.contains("16 张"));
                    assert!(events.is_empty());
                }
                assert_private(&meta, &events);
            }
        }
    }
    // Every intermediate count stays one typed event for a single message.
    for count in 17..=256 {
        let images = Value::Array((0..count).map(|_| image("codex")).collect());
        let (_, events, error) = parse_records("codex", vec![message("codex", "user", images)]);
        assert!(error.is_none(), "count={count}: {error:?}");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].media.len(), count);
        assert_eq!(events[0].message["text"], "[图片]");
    }
}

#[test]
fn invalid_external_images_still_fail_closed() {
    for source in ["claude", "codex", "grok"] {
        for block in [
            json!({"type":"image_url","image_url":"https://example.invalid/private.png"}),
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"!"}}),
        ] {
            for tool in [false, true] {
                let records = if tool {
                    tool_records(source, json!([block]))
                } else {
                    vec![message(source, "user", json!([block]))]
                };
                let (meta, events, error) = parse_records(source, records);
                assert!(error.is_some(), "{source} tool={tool}");
                assert!(events.is_empty());
                assert_private(&meta, &events);
            }
        }
    }
}

/// Batch 33: a non-image block of an unknown kind is skipped like the
/// reference `_flatten_content` — its payload never becomes text or media —
/// and the session row reports the skip. Codex tool output keeps its
/// separately reviewed envelope grammar and still fails closed there.
#[test]
fn unknown_nonimage_blocks_are_skipped_without_leaking_their_payload() {
    for source in ["claude", "codex", "grok"] {
        for block in [
            json!({"type":"audio","data":PNG}),
            json!({"type":"unknown_future_block","data":PNG}),
        ] {
            let kind = block["type"].as_str().unwrap().to_owned();
            let (meta, events, error) = parse_records(
                source,
                vec![message(source, "user", json!([block.clone()]))],
            );
            assert!(error.is_none(), "{source} {kind}: {error:?}");
            assert!(
                events.iter().all(|event| event.message["role"] != "user"),
                "{source} {kind}: an unknown block must not become a message"
            );
            assert_private(&meta, &events);
            assert_eq!(
                meta["migration_warnings"],
                json!([format!("跳过未知的内容块类型：{kind} ×1")]),
                "{source}"
            );
            let (meta, events, error) = parse_records(source, tool_records(source, json!([block])));
            if source == "codex" {
                assert!(error.is_some(), "codex tool {kind}");
                assert!(events.is_empty());
            } else {
                assert!(error.is_none(), "{source} tool {kind}: {error:?}");
                let result = events
                    .iter()
                    .find(|event| event.message["role"] == "tool_result")
                    .expect("tool result");
                assert_eq!(result.message["text"], "");
                assert!(result.media.is_empty());
                assert_eq!(
                    meta["migration_warnings"],
                    json!([format!("跳过未知的内容块类型：{kind} ×1")]),
                    "{source}"
                );
            }
            assert_private(&meta, &events);
        }
    }
}

#[test]
fn claude_file_base64_images_do_not_cross_tool_or_thinking_boundaries() {
    let image = json!({"type":"image","file":{"base64":PNG,"mimeType":"image/png"}});
    let content = json!([{"type":"text","text":"before"},image,{"type":"thinking","thinking":"reasoning"},image,
        {"type":"tool_use","id":"call","name":"capture","input":{}},image]);
    let (meta, events, error) =
        parse_records("claude", vec![message("claude", "assistant", content)]);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(
        events
            .iter()
            .map(|event| event.message["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "assistant",
            "assistant",
            "thinking",
            "assistant",
            "tool",
            "assistant"
        ]
    );
    assert_eq!(
        events
            .iter()
            .map(|event| event.media.len())
            .collect::<Vec<_>>(),
        vec![0, 1, 0, 1, 0, 1]
    );
    assert_private(&meta, &events);
}

#[test]
fn claude_abandoned_image_inputs_keep_branch_interruption_and_active_turn_identity() {
    let (meta, events, error) = parse_records(
        "claude",
        vec![
            json!({"type":"user","uuid":"root","parentUuid":null,"message":{"content":"root"}}),
            json!({"type":"user","uuid":"abandoned","parentUuid":"root","message":{"content":[image("claude")]}}),
            json!({"type":"user","uuid":"active","parentUuid":"root","message":{"content":[image("claude")]}}),
        ],
    );
    assert!(error.is_none(), "{error:?}");
    let pictures = events
        .iter()
        .filter(|event| !event.media.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(pictures.len(), 2);
    assert_eq!(pictures[0].message["interrupted"], true);
    assert_eq!(pictures[0].end, 200);
    assert_eq!(pictures[1].message["turn_id"], "active");
    assert!(pictures[1].message.get("interrupted").is_none());
    assert!(
        events
            .iter()
            .any(|event| event.end == 200 && event.message["state"] == "aborted")
    );
    assert_private(&meta, &events);
}

#[test]
fn plain_json_image_tutorials_and_tool_arguments_are_not_native_media() {
    for source in ["claude", "codex", "grok"] {
        let text =
            json!({"type":"image","image_url":"https://example.invalid/tutorial.png"}).to_string();
        let (meta, events, error) = parse_records(
            source,
            vec![message(
                source,
                "user",
                json!([{"type":"text","text":text}]),
            )],
        );
        assert!(error.is_none(), "{source}: {error:?}");
        assert_private(&meta, &events);
        assert_eq!(
            events
                .iter()
                .find(|event| event.message["role"] == "user")
                .unwrap()
                .message["text"],
            text
        );
        assert_eq!(
            events.iter().map(|event| event.media.len()).sum::<usize>(),
            0
        );
    }
    let input = json!({"config":{"image_url":"https://example.invalid/configured.png"}});
    let (meta, events, error) = parse_records(
        "claude",
        vec![message(
            "claude",
            "assistant",
            json!([{"type":"tool_use","id":"call","name":"capture","input":input}]),
        )],
    );
    assert!(error.is_none(), "{error:?}");
    assert_eq!(events[0].media.len(), 0);
    assert_eq!(events[0].message["text"], string(&input));
    assert_private(&meta, &events);
}

#[test]
fn many_newlines_do_not_trigger_json_suffix_scans_in_normal_text() {
    let literal =
        json!({"type":"image","image_url":"https://example.invalid/tutorial.png"}).to_string();
    let text = format!("{}{literal}", "\n".repeat(50_000));
    for source in ["claude", "codex", "grok"] {
        let (_, events, error) = parse_records(
            source,
            vec![message(
                source,
                "user",
                json!([{"type":"text","text":text}]),
            )],
        );
        assert!(error.is_none(), "{source}: {error:?}");
        let input = events
            .iter()
            .find(|event| event.message["role"] == "user")
            .unwrap();
        assert!(
            input.message["text"] == text,
            "{source}: ordinary text changed"
        );
        assert!(input.media.is_empty());
    }
    let (_, events, error) = parse_records("codex", tool_records("codex", json!(text)));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(events.last().unwrap().message["text"], text);
    assert!(events.last().unwrap().media.is_empty());
    let envelope = json!({"output":[image("codex")],"exit_code":0,"wall_time_seconds":0.5});
    let wrapped = format!("{}Output:\n{envelope}", "noise\n".repeat(50_000));
    let (meta, events, error) = parse_records("codex", tool_records("codex", json!(wrapped)));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(events.last().unwrap().media.len(), 1);
    assert_eq!(events.last().unwrap().message["text"], "[图片]");
    assert_private(&meta, &events);
}

#[test]
fn explicit_mcp_content_wrappers_project_media_and_keep_error_flags() {
    for source in ["claude", "codex", "grok"] {
        for marker in [false, true] {
            let mut output = json!({"content":[{"type":"text","text":"capture"},image(source)]});
            if marker {
                output["isError"] = json!(true);
            }
            let (meta, events, error) = parse_records(source, tool_records(source, output));
            assert!(error.is_none(), "{source}: {error:?}");
            let result = events
                .iter()
                .find(|event| event.message["role"] == "tool_result")
                .unwrap();
            assert_eq!(result.message["text"], "capture");
            assert_eq!(result.message["error"], marker);
            assert_eq!(result.media.len(), 1);
            assert_private(&meta, &events);
        }
    }
    let envelope = json!({"output":{"content":[image("codex")],"isError":true},"exit_code":0,"wall_time_seconds":0.1});
    let (meta, events, error) =
        parse_records("codex", tool_records("codex", json!(envelope.to_string())));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(events.last().unwrap().message["error"], true);
    assert_eq!(events.last().unwrap().media.len(), 1);
    assert_private(&meta, &events);
}

#[test]
fn business_content_arrays_stay_json_and_unmarked_mcp_json_text_is_not_decoded() {
    for output in [
        json!({"content":[{"metric":2}]}),
        json!({"content":[]}),
        json!({"content":["ordinary",42]}),
    ] {
        let (_, events, error) = parse_records("codex", tool_records("codex", output.clone()));
        assert!(error.is_none(), "{error:?}");
        assert_eq!(events.last().unwrap().message["text"], string(&output));
        assert!(events.last().unwrap().media.is_empty());
    }
    let literal = json!({"content":[{"type":"image_url","image_url":"https://example.invalid/tutorial.png"}]}).to_string();
    let (_, events, error) = parse_records("codex", tool_records("codex", json!(literal)));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(events.last().unwrap().message["text"], literal);
    assert!(events.last().unwrap().media.is_empty());
    let unknown = json!({"content":[{"type":"unknown","data":PNG}],"isError":false});
    let (meta, events, error) = parse_records("codex", tool_records("codex", unknown));
    assert!(error.is_some());
    assert!(events.is_empty());
    assert_private(&meta, &events);
}

#[test]
fn malformed_text_blocks_cannot_serialize_image_objects_into_text_or_metadata() {
    for source in ["claude", "codex", "grok"] {
        let (meta, events, error) = parse_records(
            source,
            vec![message(
                source,
                "user",
                json!({"type":"text","text":image(source)}),
            )],
        );
        assert!(error.is_some());
        assert!(events.is_empty());
        assert_private(&meta, &events);
    }
}
