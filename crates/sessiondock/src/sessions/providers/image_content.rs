//! Typed image extraction before native content becomes public text or JSON.
use super::{MediaContext, Skipped};
use crate::media::NativeImage;
use serde_json::Value;

pub(super) fn native_image(
    value: &Value,
    context: Option<&MediaContext<'_>>,
) -> Result<Option<NativeImage>, String> {
    if let Some(image) = context.and_then(|context| context.lookup(value)) {
        return Ok(Some(image));
    }
    NativeImage::from_block(value)
}

pub(super) const MAX_MEDIA: usize = 256;
const MAX_DEPTH: usize = 64;

pub(super) fn image_shape(value: &Value) -> bool {
    matches!(
        value["type"].as_str(),
        Some("image" | "input_image" | "image_url")
    ) || value.get("image_url").is_some()
        || (value["type"] == "file"
            && ["media_type", "mime_type", "mimeType"].iter().any(|key| {
                value["file"][*key]
                    .as_str()
                    .is_some_and(|mime| mime.starts_with("image/"))
                    || value[*key]
                        .as_str()
                        .is_some_and(|mime| mime.starts_with("image/"))
            }))
}

fn push(media: &mut Vec<NativeImage>, image: NativeImage) -> Result<(), String> {
    if media.len() >= MAX_MEDIA {
        return Err("单条消息内嵌图片超过 256 张限制".into());
    }
    media.push(image);
    Ok(())
}

pub(super) fn parts_with_media(
    value: &Value,
    context: Option<&MediaContext<'_>>,
    skipped: &mut Skipped,
) -> Result<(String, Vec<NativeImage>), String> {
    let (value, media) = sanitize_with_media(value, context)?;
    let text = super::text_parts(&value, skipped)?;
    Ok((placeholder(text, &media), media))
}

pub(super) fn placeholder(text: String, media: &[NativeImage]) -> String {
    if text.is_empty() && !media.is_empty() {
        "[图片]".into()
    } else {
        text
    }
}

fn sanitize_with_media(
    value: &Value,
    context: Option<&MediaContext<'_>>,
) -> Result<(Value, Vec<NativeImage>), String> {
    let mut media = Vec::new();
    let cleaned = clean(value, &mut media, 0, context)?.unwrap_or_else(|| Value::Array(Vec::new()));
    Ok((cleaned, media))
}

pub(super) fn tool_wrapper(value: &Value) -> bool {
    tool_wrapper_with_media(value, None)
}
pub(super) fn tool_wrapper_with_media(value: &Value, context: Option<&MediaContext<'_>>) -> bool {
    if !value.is_object() || value.get("type").is_some() {
        return false;
    }
    let Some(items) = value["content"].as_array() else {
        return false;
    };
    value["isError"].is_boolean()
        || (!items.is_empty()
            && items.iter().all(|part| {
                matches!(
                    part["type"].as_str(),
                    Some("text" | "input_text" | "output_text" | "summary_text")
                ) || image_shape(part)
                    || context.and_then(|context| context.lookup(part)).is_some()
            }))
}

/// MCP's structural tool-output wrapper is distinct from arbitrary JSON fields
/// or JSON text. Visit only its content array, never structuredContent/metadata.
pub(super) fn sanitize_tool_with_media(
    value: &Value,
    context: Option<&MediaContext<'_>>,
) -> Result<(Value, Vec<NativeImage>), String> {
    if !tool_wrapper_with_media(value, context) {
        return sanitize_with_media(value, context);
    }
    let (mut content, media) = sanitize_with_media(&value["content"], context)?;
    if content.as_array().is_some_and(Vec::is_empty) && !media.is_empty() {
        // Retain the recognized MCP wrapper after removing its image-only
        // content. An ordinary business {content:[]} is still not a wrapper.
        content = serde_json::json!([{"type":"text","text":""}]);
    }
    // Early hard-failure check only (non-string text, invalid shapes); unknown
    // non-image blocks are counted once by the caller's text projection.
    super::text_parts(&content, &mut Skipped::default())?;
    let mut wrapper = value.clone();
    wrapper["content"] = content;
    Ok((wrapper, media))
}

fn clean(
    value: &Value,
    media: &mut Vec<NativeImage>,
    depth: usize,
    context: Option<&MediaContext<'_>>,
) -> Result<Option<Value>, String> {
    if depth > MAX_DEPTH {
        return Err("原生媒体内容嵌套超过 64 层限制".into());
    }
    // Only native block boundaries carry media authority. Text (including JSON
    // tutorials and literal data URLs) and arbitrary object fields are data.
    if matches!(
        value["type"].as_str(),
        Some("text" | "input_text" | "output_text" | "summary_text")
    ) {
        return Ok(Some(value.clone()));
    }
    if let Some(image) = native_image(value, context)? {
        push(media, image)?;
        // Images are private typed payloads, not additional text lines. The
        // public message adds one placeholder only if no real text remains.
        return Ok(None);
    }
    match value {
        Value::Array(items) => {
            let before = media.len();
            let cleaned = items
                .iter()
                .map(|item| clean(item, media, depth + 1, context))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            if cleaned.is_empty() && media.len() > before {
                Ok(None)
            } else {
                Ok(Some(Value::Array(cleaned)))
            }
        }
        _ => Ok(Some(value.clone())),
    }
}

/// Claude emits an event per native content block, including each image. Keep
/// that count/order while retaining the existing contiguous-run image budget.
pub(super) fn validate_claude(events: &[super::Event]) -> Result<(), String> {
    let mut previous: Option<&super::Event> = None;
    let mut count = 0;
    for event in events {
        let same = previous.is_some_and(|previous| {
            if previous.end != event.end {
                return false;
            }
            let role = event.message["role"].as_str().unwrap_or("");
            if !matches!(
                role,
                "user" | "assistant" | "user·subagent" | "assistant·subagent"
            ) {
                return false;
            }
            let previous = previous.message.as_object().expect("message object");
            let current = event.message.as_object().expect("message object");
            previous.keys().filter(|key| key.as_str() != "text").count()
                == current.keys().filter(|key| key.as_str() != "text").count()
                && previous
                    .iter()
                    .filter(|(key, _)| key.as_str() != "text")
                    .all(|(key, value)| current.get(key) == Some(value))
        });
        if !same {
            count = 0;
        }
        count += event.media.len();
        if count > MAX_MEDIA {
            return Err("单条消息内嵌图片超过 256 张限制".into());
        }
        previous = Some(event);
    }
    Ok(())
}
