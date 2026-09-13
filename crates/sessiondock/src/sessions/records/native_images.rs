//! Private structural image authority. JSON paths only locate already-reviewed
//! native content blocks; a JSON value cannot manufacture a sidecar.
use super::scanner::{Node, Text, TextSpan};
use crate::media::NativeImage;
use crate::native_replay::{CheckedReplay, DecodePlan};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PathPart {
    Key(String),
    Index(usize),
}
#[derive(Clone)]
pub(crate) struct Sidecar {
    pub image: Option<NativeImage>,
    pub tool_error: Option<bool>,
    pub path: Vec<PathPart>,
}
impl Sidecar {
    pub(crate) fn retained_weight(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.image.as_ref().map_or(0, NativeImage::resident_len))
            .saturating_add(
                self.path
                    .capacity()
                    .saturating_mul(std::mem::size_of::<PathPart>()),
            )
            .saturating_add(
                self.path
                    .iter()
                    .map(|part| match part {
                        PathPart::Key(key) => key.capacity(),
                        PathPart::Index(_) => 0,
                    })
                    .sum::<usize>(),
            )
    }
}
pub(crate) struct SpanImage {
    pub mime: String,
    pub span: TextSpan,
    pub encoded_offset: u64,
    pub payload_sha1: [u8; 20],
    pub plan: Option<DecodePlan>,
}

/// Addresses are derived only from final borrowed records. The lifetime prevents
/// moving/mutating their trees while this non-serializable context is in use.
pub(crate) struct MediaContext<'a> {
    images: HashMap<usize, NativeImage>,
    tool_errors: HashMap<usize, bool>,
    _records: PhantomData<&'a Value>,
}
impl<'a> MediaContext<'a> {
    pub(crate) fn new(
        records: &'a [(Value, u64)],
        sidecars: &BTreeMap<u64, Vec<Sidecar>>,
    ) -> Result<Self, String> {
        let mut images = HashMap::new();
        let mut tool_errors = HashMap::new();
        for (record, end) in records {
            for sidecar in sidecars.get(end).into_iter().flatten() {
                let mut value = record;
                for part in &sidecar.path {
                    value = match part {
                        PathPart::Key(key) => value.get(key),
                        PathPart::Index(index) => value.get(*index),
                    }
                    .ok_or_else(invalid)?;
                }
                let pointer = value as *const Value as usize;
                match (&sidecar.image, sidecar.tool_error) {
                    (Some(image), None) => {
                        if images.insert(pointer, image.clone()).is_some() {
                            return Err(invalid());
                        }
                    }
                    (None, Some(error)) => {
                        if tool_errors.insert(pointer, error).is_some() {
                            return Err(invalid());
                        }
                    }
                    _ => return Err(invalid()),
                }
            }
        }
        Ok(Self {
            images,
            tool_errors,
            _records: PhantomData,
        })
    }
    pub(crate) fn lookup(&self, value: &Value) -> Option<NativeImage> {
        self.images.get(&(value as *const Value as usize)).cloned()
    }
    pub(crate) fn tool_error(&self, value: &Value) -> Option<bool> {
        self.tool_errors
            .get(&(value as *const Value as usize))
            .copied()
    }
}

fn invalid() -> String {
    "原生媒体结构、编码别名或私有位置无效".into()
}
fn text(node: &Node) -> Option<&str> {
    match node {
        Node::String(Text::Inline(value)) => Some(value),
        _ => None,
    }
}
fn get<'a>(node: &'a Node, key: &str) -> Option<&'a Node> {
    match node {
        Node::Object(values) => values.get(key),
        _ => None,
    }
}
fn kind(node: &Node) -> &str {
    get(node, "type").and_then(text).unwrap_or("")
}
fn image(node: &Node) -> bool {
    matches!(kind(node), "image" | "input_image" | "image_url")
        || get(node, "image_url").is_some()
        || (kind(node) == "file" && matches!(get(node, "file"), Some(Node::Object(_))))
}
fn mime(value: &str) -> Result<String, String> {
    Ok(match value.trim().to_ascii_lowercase().as_str() {
        "image/png" | "image/apng" => "image/png",
        "image/jpeg" | "image/jpg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "image/avif" => "image/avif",
        "image/bmp" | "image/x-ms-bmp" => "image/bmp",
        _ => return Err("此原生媒体格式尚未支持".into()),
    }
    .into())
}

#[cfg(test)]
pub(crate) fn prepare(
    source: &str,
    root: Node,
    make_span: impl FnMut(SpanImage) -> Result<NativeImage, String>,
) -> Result<(Value, Vec<Sidecar>), String> {
    prepare_with_replay(
        source,
        root,
        make_span,
        |_| Err("原生工具重放来源未配置".into()),
        |_| Err("原生文本读回来源未配置".into()),
    )
}

/// `materialize` reads one remaining ordinary giant string back from the
/// caller's checked source, regardless of record size; a large string is
/// still not image authority. It only ever sees
/// top-level spans: nested envelope text addresses decoded layers, not file
/// bytes, and stays an explicit failure.
pub(crate) fn prepare_with_replay(
    source: &str,
    mut root: Node,
    mut make_span: impl FnMut(SpanImage) -> Result<NativeImage, String>,
    mut open: impl FnMut(&DecodePlan) -> Result<Box<dyn CheckedReplay>, String>,
    mut materialize: impl FnMut(&TextSpan) -> Result<String, String>,
) -> Result<(Value, Vec<Sidecar>), String> {
    let route: &[&str] = match (source, kind(&root)) {
        ("claude", "user" | "assistant") => &["message", "content"],
        ("codex", "response_item") => match get(&root, "payload").map(kind).unwrap_or("") {
            "message" => &["payload", "content"],
            "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
                &["payload", "output"]
            }
            _ => &[],
        },
        ("grok", "user" | "assistant" | "system" | "tool_result") => &["content"],
        _ => &[],
    };
    let tool =
        (source == "grok" && kind(&root) == "tool_result") || route.last() == Some(&"output");
    let mut path = Vec::new();
    let mut sidecars = Vec::new();
    fn routed<'a>(
        mut node: &'a mut Node,
        route: &[&str],
        path: &mut Vec<PathPart>,
    ) -> Option<&'a mut Node> {
        for key in route {
            let Node::Object(values) = node else {
                return None;
            };
            path.push(PathPart::Key((*key).into()));
            node = values.get_mut(*key)?;
        }
        Some(node)
    }
    if !route.is_empty()
        && let Some(node) = routed(&mut root, route, &mut path)
    {
        Walker {
            sidecars: &mut sidecars,
            make: &mut make_span,
            open: &mut open,
            codex: source == "codex",
        }
        .walk(node, &mut path, tool, 0, 0, None)?;
    }
    root.materialize_spans(&mut materialize)?;
    Ok((
        root.into_value().map_err(|error| error.to_string())?,
        sidecars,
    ))
}

struct Walker<'a, M, O> {
    sidecars: &'a mut Vec<Sidecar>,
    make: &'a mut M,
    open: &'a mut O,
    codex: bool,
}
impl<M, O> Walker<'_, M, O>
where
    M: FnMut(SpanImage) -> Result<NativeImage, String>,
    O: FnMut(&DecodePlan) -> Result<Box<dyn CheckedReplay>, String>,
{
    fn walk(
        &mut self,
        node: &mut Node,
        path: &mut Vec<PathPart>,
        tool: bool,
        depth: usize,
        envelopes: usize,
        origin: Option<(&DecodePlan, u64)>,
    ) -> Result<(), String> {
        // A multi-part Codex output (`[header, chunk, chunk, …]`) is not one
        // candidate: the array walk below visits each part as its own
        // reviewed position, so a giant chunk is decoded in place and a giant
        // ordinary part stays a top-level span for the caller to read back.
        let multipart = depth == 0 && matches!(node, Node::Array(values) if values.len() > 1);
        if self.codex
            && tool
            && !multipart
            && let Some(candidate) = super::tool_envelopes::candidate(node)?
        {
            let tool_error = candidate.tool_error;
            let parsed = super::tool_envelopes::parse(candidate.span, origin, self.open)?;
            let Some(parsed) = parsed else {
                // Ordinary giant tool text: a top-level span is read back by
                // the caller after the walk; nested text has no file range.
                return Ok(());
            };
            *node = parsed.root;
            if let Some(error) = tool_error {
                self.push(Sidecar {
                    image: None,
                    tool_error: Some(error),
                    path: path.clone(),
                })?;
            }
            let origin = Some((&parsed.plan, parsed.candidate_start));
            self.walk(node, path, tool, depth, envelopes, origin)?;
            // A nested span addresses decoded parent text. Replay its entire
            // validated plan instead of treating it as a physical file offset.
            node.materialize_spans(&mut |span| {
                use std::io::Read;
                let plan = super::tool_envelopes::extend(origin, span)?;
                let mut reader = (self.open)(&plan)?;
                let mut text = String::new();
                let read = reader
                    .read_to_string(&mut text)
                    .map_err(|_| "嵌套原生文本读取或验证失败".to_owned());
                let finished = reader.finish();
                read?;
                finished?;
                Ok(text)
            })?;
            return Ok(());
        }
        // A text block remains ordinary text even when a tutorial/extension adds
        // an image_url field. Match the provider's text-first structural grammar.
        if matches!(
            kind(node),
            "text" | "input_text" | "output_text" | "summary_text"
        ) {
            return Ok(());
        }
        if image(node) {
            if let Some(image) = extract(node, &mut |mut image| {
                if let Some(origin) = origin {
                    image.plan = Some(super::tool_envelopes::extend(Some(origin), &image.span)?);
                }
                (self.make)(image)
            })? {
                let sidecar = Sidecar {
                    image: Some(image),
                    tool_error: None,
                    path: path.clone(),
                };
                self.push(sidecar)?;
            }
            return Ok(());
        }
        if let Node::Array(values) = node {
            for (index, value) in values.iter_mut().enumerate() {
                path.push(PathPart::Index(index));
                self.walk(value, path, tool, depth + 1, envelopes, origin)?;
                path.pop();
            }
            return Ok(());
        }
        let mcp = tool
            && get(node, "type").is_none()
            && matches!(get(node, "content"), Some(Node::Array(values)) if
            matches!(get(node, "isError"), Some(Node::Bool(_))) || (!values.is_empty() && values.iter().all(|v| image(v) || matches!(kind(v), "text" | "input_text" | "output_text" | "summary_text"))));
        let child = if kind(node) == "tool_result" || mcp {
            Some(("content", true, envelopes))
        } else if tool
            && get(node, "output").is_some()
            && get(node, "wall_time_seconds").is_some()
            && ["exit_code", "session_id", "chunk_id"]
                .iter()
                .any(|key| get(node, key).is_some())
        {
            Some(("output", true, envelopes + 1))
        } else {
            None
        };
        if let Some((key, child_tool, envelopes)) = child
            && let Node::Object(values) = node
            && let Some(value) = values.get_mut(key)
        {
            path.push(PathPart::Key(key.into()));
            self.walk(value, path, child_tool, depth + 1, envelopes, origin)?;
            path.pop();
        }
        Ok(())
    }
    fn push(&mut self, sidecar: Sidecar) -> Result<(), String> {
        self.sidecars.push(sidecar);
        Ok(())
    }
}

struct Payload {
    path: Vec<String>,
    mime: String,
    hash: [u8; 20],
    offset: u64,
    span: bool,
}
fn payload(
    node: &Node,
    path: Vec<String>,
    declared: Option<&str>,
    url: bool,
) -> Result<Payload, String> {
    let (prefix, len, full_hash, is_span) = match node {
        Node::String(Text::Inline(value)) => (
            value.as_bytes(),
            value.len() as u64,
            Sha1::digest(value.as_bytes()).into(),
            false,
        ),
        Node::String(Text::Span(span)) => (span.prefix(), span.decoded_len(), *span.digest(), true),
        _ => return Err(invalid()),
    };
    let (mime, offset, hash) = if url {
        let comma = prefix.iter().position(|b| *b == b',').ok_or_else(invalid)?;
        let head = std::str::from_utf8(&prefix[..comma]).map_err(|_| invalid())?;
        let mime = mime(
            head.strip_prefix("data:")
                .and_then(|s| s.strip_suffix(";base64"))
                .ok_or_else(invalid)?,
        )?;
        let offset = (comma + 1) as u64;
        let hash = match node {
            Node::String(Text::Span(span)) => {
                let (candidate, hash) = span.data_suffix().ok_or_else(invalid)?;
                if candidate != offset {
                    return Err(invalid());
                }
                hash
            }
            _ => Sha1::digest(&prefix[comma + 1..]).into(),
        };
        (mime, offset, hash)
    } else {
        (mime(declared.ok_or_else(invalid)?)?, 0, full_hash)
    };
    if len <= offset {
        return Err(invalid());
    }
    Ok(Payload {
        path,
        mime,
        hash,
        offset,
        span: is_span,
    })
}

fn strip_spanned_aliases(node: &mut Node) {
    fn strip(object: &mut Node) {
        let Node::Object(values) = object else {
            return;
        };
        for key in ["data", "base64", "url", "image_url"] {
            if matches!(values.get(key), Some(Node::String(Text::Span(_)))) {
                values.shift_remove(key);
            } else if matches!(key, "url" | "image_url")
                && let Some(Node::Object(nested)) = values.get_mut(key)
                && matches!(nested.get("url"), Some(Node::String(Text::Span(_))))
            {
                nested.shift_remove("url");
            }
        }
    }
    if let Node::Object(values) = node {
        for key in ["source", "file"] {
            if let Some(object) = values.get_mut(key) {
                strip(object);
            }
        }
    }
    strip(node);
}

fn extract(
    node: &mut Node,
    make: &mut impl FnMut(SpanImage) -> Result<NativeImage, String>,
) -> Result<Option<NativeImage>, String> {
    let mut selected = None;
    'objects: for wrapper in [Some("source"), Some("file"), None] {
        let object = match wrapper {
            Some(key) => match get(node, key) {
                Some(value) => value,
                None => continue,
            },
            None => &*node,
        };
        let Node::Object(values) = object else {
            continue;
        };
        let prefix: Vec<String> = wrapper.into_iter().map(str::to_owned).collect();
        // Python media.from_block uses `or` for aliases and returns after the
        // first usable source. Later conflicting spellings are never a reason
        // to reject the record.
        let data = ["data", "base64"].into_iter().find_map(|key| {
            values.get(key).and_then(|value| match value {
                Node::String(Text::Inline(text)) if !text.is_empty() => Some((key, value)),
                Node::String(Text::Span(span)) if span.decoded_len() != 0 => Some((key, value)),
                _ => None,
            })
        });
        if let Some((key, value)) = data {
            let declared = ["media_type", "mime_type", "mimeType", "type"]
                .into_iter()
                .find_map(|key| {
                    values
                        .get(key)
                        .and_then(text)
                        .filter(|value| !value.is_empty())
                })
                .and_then(|value| mime(value).ok());
            let mut path = prefix.clone();
            path.push(key.into());
            if let Some(declared) = declared
                && let Ok(payload) = payload(value, path, Some(&declared), false)
            {
                if payload.span {
                    selected = Some(payload);
                    break 'objects;
                }
                return Ok(None);
            }
        }

        let Some((key, mut value)) = ["url", "image_url"].into_iter().find_map(|key| {
            values.get(key).and_then(|value| match value {
                Node::String(Text::Inline(text)) if !text.is_empty() => Some((key, value)),
                Node::String(Text::Span(span)) if span.decoded_len() != 0 => Some((key, value)),
                Node::Object(_) => Some((key, value)),
                _ => None,
            })
        }) else {
            continue;
        };
        let mut path = prefix;
        path.push(key.into());
        if matches!(value, Node::Object(_)) {
            let Some(nested) = get(value, "url") else {
                continue;
            };
            value = nested;
            path.push("url".into());
        }
        if matches!(value, Node::String(Text::Inline(_))) {
            // The ordinary value projector applies the same remote, data URL,
            // and local-path parsing once the record becomes serde_json.
            return Ok(None);
        }
        if let Ok(payload) = payload(value, path, None, true) {
            selected = Some(payload);
            break 'objects;
        }
    }

    let payload = match selected {
        Some(payload) => payload,
        None => return Ok(None),
    };
    let mut parent = &mut *node;
    for key in &payload.path[..payload.path.len() - 1] {
        let Node::Object(values) = parent else {
            return Err(invalid());
        };
        parent = values.get_mut(key).ok_or_else(invalid)?;
    }
    let Node::Object(values) = parent else {
        return Err(invalid());
    };
    let removed = values
        .shift_remove(payload.path.last().ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    let Node::String(Text::Span(span)) = removed else {
        return Ok(None);
    };
    // Later aliases are ignored by Python's first-match parser. Remove only
    // their private scanner spans so they are neither materialized as public
    // JSON nor treated as validation constraints.
    strip_spanned_aliases(node);
    // The real block remains at the same path, with payload fields removed.
    // Only the private context can interpret it; there is no serialized marker.
    make(SpanImage {
        mime: payload.mime,
        span,
        encoded_offset: payload.offset,
        payload_sha1: payload.hash,
        plan: None,
    })
    .map(Some)
}

#[cfg(test)]
mod tests;
