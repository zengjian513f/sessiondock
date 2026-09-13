use super::super::scanner::{Limits, scan};
use super::*;
use crate::media::NativeSpan;
use serde_json::json;
use std::path::PathBuf;

const DATA: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
fn synthetic_root() -> PathBuf {
    std::env::temp_dir().join("sessiondock-native-image-fixture")
}
fn synthetic_path(name: &str) -> PathBuf {
    synthetic_root().join(name)
}
fn image_block() -> Value {
    json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":DATA}})
}
fn prepare_value(source: &str, value: Value) -> Result<(Value, Vec<Sidecar>), String> {
    let bytes = serde_json::to_vec(&value).unwrap();
    let document = scan(
        bytes.as_slice(),
        Limits {
            inline_string_bytes: 64,
        },
    )
    .unwrap();
    prepare(source, document.root, |image| {
        NativeImage::from_native_span(NativeSpan {
            root: synthetic_root(),
            path: synthetic_path("session.jsonl"),
            file_identity: "fixture".into(),
            record_start: 0,
            record_end: bytes.len() as u64,
            start: image.span.start(),
            end: image.span.end(),
            plan: image.plan,
            decoded_len: image.span.decoded_len(),
            decoded_sha1: *image.span.digest(),
            encoded_offset: image.encoded_offset,
            payload_sha1: image.payload_sha1,
            mime: image.mime,
        })
        .map_err(|error| error.to_string())
    })
}
fn project(source: &str, value: Value) -> (Vec<crate::sessions::Event>, Vec<Sidecar>) {
    let (value, sidecars) = prepare_value(source, value).unwrap();
    assert!(!serde_json::to_string(&value).unwrap().contains(DATA));
    let records = vec![(value, 999)];
    let map = BTreeMap::from([(999, sidecars.clone())]);
    let (_, events, error) = crate::sessions::providers::parse_with_media(
        source,
        &synthetic_path("session.jsonl"),
        &records,
        None,
        "fixture",
        &map,
    );
    assert_eq!(error, None);
    (events, sidecars)
}

#[test]
fn three_provider_messages_keep_private_span_media_and_text() {
    for (source, value) in [
        (
            "claude",
            json!({"type":"user","uuid":"u1","message":{"content":[{"type":"text","text":"before"},image_block(),{"type":"text","text":"after"}]}}),
        ),
        (
            "codex",
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"before"},image_block()]}}),
        ),
        (
            "grok",
            json!({"type":"user","content":[{"type":"text","text":"before"},image_block()]}),
        ),
    ] {
        let (events, sidecars) = project(source, value);
        assert_eq!(sidecars.len(), 1, "{source}");
        assert_eq!(
            events.iter().map(|e| e.media.len()).sum::<usize>(),
            1,
            "{source}"
        );
        assert!(events.iter().any(|e| {
            e.message["text"]
                .as_str()
                .is_some_and(|s| s.contains("before"))
        }));
        assert!(events.iter().all(|e| !e.message.to_string().contains(DATA)));
    }
}

#[test]
fn nested_tool_output_extracts_before_clone_preserves_metadata() {
    let (events, _) = project(
        "codex",
        json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":{
            "wall_time_seconds":0.25,"exit_code":4,"output":{"content":[{"type":"text","text":"tool text"},image_block()],"isError":true}
        }}}),
    );
    let event = events.iter().find(|event| !event.media.is_empty()).unwrap();
    assert_eq!(event.message["call_id"], "c1");
    assert_eq!(event.message["exit_code"], 4);
    assert_eq!(event.message["duration_s"], 0.25);
    assert_eq!(event.message["error"], true);
    assert_eq!(event.message["text"], "tool text");
    let (events, _) = project(
        "claude",
        json!({"type":"user","uuid":"u","message":{"content":[{"type":"tool_result","tool_use_id":"c","content":{"content":[image_block()],"isError":true}}]}}),
    );
    assert_eq!(events.iter().map(|e| e.media.len()).sum::<usize>(), 1);
}

#[test]
fn huge_data_url_and_equivalent_aliases_share_semantics() {
    let data_url = format!("data:image/png;base64,{DATA}");
    let (_, data) =
        prepare_value("grok", json!({"type":"user","content":[image_block()]})).unwrap();
    let (_, url) = prepare_value(
        "grok",
        json!({"type":"user","content":[{"type":"input_image","image_url":data_url}]}),
    )
    .unwrap();
    assert_eq!(
        data[0].image.as_ref().unwrap().semantic_key(),
        url[0].image.as_ref().unwrap().semantic_key()
    );
    assert_eq!(
        url[0]
            .image
            .as_ref()
            .unwrap()
            .native_span()
            .unwrap()
            .encoded_offset,
        22
    );
    let mut block = image_block();
    block["image_url"] = json!(data_url);
    let (value, aliases) = prepare_value("grok", json!({"type":"user","content":[block]})).unwrap();
    assert_eq!(aliases.len(), 1);
    assert!(!value.to_string().contains(DATA));
    assert_eq!(
        data[0].image.as_ref().unwrap().semantic_key(),
        aliases[0].image.as_ref().unwrap().semantic_key()
    );
}

#[test]
fn later_conflicting_aliases_and_paths_do_not_override_the_first_image_source() {
    for extra in [
        json!({"image_url":format!("data:image/jpeg;base64,{DATA}")}),
        json!({"base64":"X".repeat(96),"media_type":"image/png"}),
        json!({"path":"/tmp/a.png"}),
    ] {
        let mut block = image_block();
        block
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let (_, sidecars) =
            prepare_value("grok", json!({"type":"user","content":[block]})).unwrap();
        assert_eq!(sidecars.len(), 1);
        assert_eq!(
            sidecars[0].image.as_ref().unwrap().encoded_len(),
            DATA.len()
        );
    }
}

#[test]
fn sidecar_retained_weight_counts_spare_path_and_key_capacity() {
    let (_, mut sidecars) =
        prepare_value("grok", json!({"type":"user","content":[image_block()]})).unwrap();
    let sidecar = &mut sidecars[0];
    let before = sidecar.retained_weight();
    sidecar.path.reserve(100);
    let mut key = String::with_capacity(4096);
    key.push_str("key");
    sidecar.path.push(PathPart::Key(key));
    assert!(sidecar.retained_weight() >= before + 4096);
    assert!(
        sidecar.retained_weight()
            >= sidecar.path.capacity() * std::mem::size_of::<PathPart>()
                + 4096
                + sidecar.image.as_ref().unwrap().resident_len()
    );
}

#[test]
fn per_record_sidecars_exceed_former_count_without_rejecting_history() {
    let (_, sidecars) = prepare_value(
        "grok",
        json!({"type":"user","content":vec![image_block();257]}),
    )
    .unwrap();
    assert_eq!(sidecars.len(), 257);
}

#[test]
fn structured_giant_data_url_streams_into_only_small_private_sidecar() {
    use std::io::{self, Cursor, Read};
    let encoded = (32 * 1024 * 1024u64).div_ceil(3) * 4;
    let prefix = b"{\"type\":\"user\",\"content\":[{\"type\":\"input_image\",\"image_url\":\"data:image/png;base64,";
    let suffix = b"=\"}]}";
    let source = Cursor::new(prefix)
        .chain(io::repeat(b'A').take(encoded - 1))
        .chain(Cursor::new(suffix));
    let document = scan(source, Limits::default()).unwrap();
    assert!(document.stats.peak_resident_bytes < 100 * 1024);
    let (value, sidecars) = prepare("grok", document.root, |image| {
        NativeImage::from_native_span(NativeSpan {
            root: synthetic_root(),
            path: synthetic_path("source.jsonl"),
            file_identity: "fixture".into(),
            record_start: 0,
            record_end: prefix.len() as u64 + encoded - 1 + suffix.len() as u64,
            start: image.span.start(),
            end: image.span.end(),
            plan: image.plan,
            decoded_len: image.span.decoded_len(),
            decoded_sha1: *image.span.digest(),
            encoded_offset: image.encoded_offset,
            payload_sha1: image.payload_sha1,
            mime: image.mime,
        })
        .map_err(|e| e.to_string())
    })
    .unwrap();
    assert!(value.to_string().len() < 256);
    assert_eq!(sidecars.len(), 1);
    assert_eq!(
        sidecars[0].image.as_ref().unwrap().encoded_len(),
        encoded as usize
    );
    assert!(sidecars[0].retained_weight() < 4096);
}

#[test]
fn context_is_not_recreated_by_cloning_or_json_fields() {
    let (value, sidecars) =
        prepare_value("grok", json!({"type":"user","content":[image_block()]})).unwrap();
    let records = vec![(value, 1)];
    let sidecars = BTreeMap::from([(1, sidecars)]);
    let context = MediaContext::new(&records, &sidecars).unwrap();
    let original = &records[0].0["content"][0];
    assert!(context.lookup(original).is_some());
    assert!(context.lookup(&original.clone()).is_none());
    assert!(context.lookup(&json!({"_native_span":true})).is_none());
    let mut invalid = sidecars.clone();
    invalid.get_mut(&1).unwrap()[0]
        .path
        .push(PathPart::Index(123));
    assert!(MediaContext::new(&records, &invalid).is_err());
}

#[test]
fn context_rejects_empty_mixed_and_duplicate_error_provenance() {
    let records = vec![(json!({"output":"text"}), 1)];
    let error = Sidecar {
        image: None,
        tool_error: Some(false),
        path: vec![],
    };
    let map = BTreeMap::from([(1, vec![error.clone()])]);
    let context = MediaContext::new(&records, &map).unwrap();
    assert_eq!(context.tool_error(&records[0].0), Some(false));
    assert_eq!(context.tool_error(&records[0].0.clone()), None);
    let empty = Sidecar {
        image: None,
        tool_error: None,
        path: vec![],
    };
    assert!(MediaContext::new(&records, &BTreeMap::from([(1, vec![empty])])).is_err());
    for value in [false, true] {
        let mut duplicate = error.clone();
        duplicate.tool_error = Some(value);
        assert!(
            MediaContext::new(
                &records,
                &BTreeMap::from([(1, vec![error.clone(), duplicate])])
            )
            .is_err()
        );
    }
    let (_, mut images) =
        prepare_value("grok", json!({"type":"user","content":[image_block()]})).unwrap();
    images[0].path = vec![];
    images[0].tool_error = Some(true);
    assert!(MediaContext::new(&records, &BTreeMap::from([(1, images)])).is_err());
}

#[test]
fn mcp_without_type_remains_authorized_after_payload_field_removal() {
    let (events, _) = project(
        "grok",
        json!({"type":"tool_result","content":{"content":[{"image_url":format!("data:image/png;base64,{DATA}")}]} }),
    );
    assert_eq!(events.iter().map(|e| e.media.len()).sum::<usize>(), 1);
    assert!(events.iter().any(|e| e.message["text"] == "[图片]"));
}

#[test]
fn spanned_and_inline_images_can_exceed_256() {
    fn parse(content: Vec<Value>) -> (Vec<crate::sessions::Event>, Option<String>) {
        let (value, sidecars) =
            prepare_value("grok", json!({"type":"user","content":content})).unwrap();
        let (_, events, error) = crate::sessions::providers::parse_with_media(
            "grok",
            &synthetic_path("session.jsonl"),
            &[(value, 1)],
            None,
            "fixture",
            &BTreeMap::from([(1, sidecars)]),
        );
        (events, error)
    }
    let mut content = vec![image_block(); 256];
    let (events, error) = parse(content.clone());
    assert!(error.is_none(), "{error:?}");
    assert_eq!(events.iter().map(|e| e.media.len()).sum::<usize>(), 256);
    assert!(
        events
            .iter()
            .all(|e| e.media.iter().all(|i| i.native_span().is_some()))
    );
    // Another inline image survives alongside the 256 private spans.
    content.push(
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}),
    );
    let (events, error) = parse(content);
    assert!(error.is_none(), "{error:?}");
    assert_eq!(events.iter().map(|e| e.media.len()).sum::<usize>(), 257);
}
