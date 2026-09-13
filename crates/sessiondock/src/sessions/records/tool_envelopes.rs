//! Streaming discovery of a known Codex tool envelope inside ONE
//! native JSON string. No file opening, ordinary-text guessing or public marker.
use super::scanner::{self, ErrorKind, Node, Text, TextSpan};
use crate::native_replay::{CheckedReplay, DecodePlan, StringRange};
use std::io::{self, Read};

pub(super) fn known(node: &Node) -> bool {
    let Node::Object(values) = node else {
        return false;
    };
    values.contains_key("output")
        && values.contains_key("wall_time_seconds")
        && ["exit_code", "session_id", "chunk_id"]
            .iter()
            .any(|key| values.contains_key(*key))
}

pub(super) fn extend(
    plan: Option<(&DecodePlan, u64)>,
    span: &TextSpan,
) -> Result<DecodePlan, String> {
    let mut ranges = plan.map_or_else(Vec::new, |(plan, _)| plan.ranges().to_vec());
    let base = plan.map_or(0, |(_, base)| base);
    ranges.push(StringRange {
        start: base.checked_add(span.start()).ok_or("原生工具区段溢出")?,
        end: base.checked_add(span.end()).ok_or("原生工具区段溢出")?,
        decoded_len: span.decoded_len(),
        decoded_sha1: *span.digest(),
    });
    DecodePlan::new(ranges).map_err(|_| "原生工具解码区段无效".into())
}

pub(super) struct Envelope {
    pub root: Node,
    pub plan: DecodePlan,
    /// Scanner offsets are relative to this candidate, while the next plan
    /// range must use offsets in the WHOLE parent decoded string.
    pub candidate_start: u64,
}

/// `Ok(None)` means the whole string was scanned and contains no supported
/// envelope: it is ordinary giant text for the caller's own materialization,
/// never an image or an empty placeholder. Source and structural
/// errors stay fatal.
pub(super) fn parse(
    span: &TextSpan,
    parent: Option<(&DecodePlan, u64)>,
    open: &mut impl FnMut(&DecodePlan) -> Result<Box<dyn CheckedReplay>, String>,
) -> Result<Option<Envelope>, String> {
    let plan = extend(parent, span)?;
    let mut reader = open(&plan)?;
    let scanned = candidates(&mut reader);
    let cleanup = finish(reader);
    cleanup?;
    let candidates = scanned?;
    for (start, end) in candidates {
        let mut reader = open(&plan)?;
        let decoded = (|| -> Result<_, String> {
            let skipped = io::copy(&mut reader.by_ref().take(start), &mut io::sink())
                .map_err(|_| "原生工具候选读取失败".to_owned())?;
            if skipped != start {
                return Err("原生工具候选区段不足".into());
            }
            let mut input = reader.by_ref().take(end - start);
            Ok(scanner::scan(
                &mut input,
                scanner::Limits::native(2 * 1024 * 1024),
            ))
        })();
        // Even failed syntax candidates must drain/finish the entire source.
        // Source errors are fatal and never become candidate misses.
        finish(reader)?;
        match decoded? {
            Ok(document) if known(&document.root) => {
                return Ok(Some(Envelope {
                    root: document.root,
                    plan,
                    candidate_start: start,
                }));
            }
            Ok(_) => {}
            Err(error) if error.kind == ErrorKind::Syntax => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(None)
}

fn finish(mut reader: Box<dyn CheckedReplay>) -> Result<(), String> {
    let drained =
        io::copy(&mut reader, &mut io::sink()).map_err(|_| "原生工具来源验证失败".to_owned());
    let finished = reader.finish();
    drained.and(finished)
}

fn candidates(reader: &mut impl Read) -> Result<Vec<(u64, u64)>, String> {
    let mut buffer = [0; 8192];
    let mut offset = 0u64;
    let mut marker_match = 0;
    let mut marker: Option<Option<u64>> = None;
    let mut first = None;
    let mut last = 0;
    let mut line_head = true;
    let mut lines = Vec::new();
    let mut scalar = [0u8; 4];
    let mut scalar_len = 0;
    let mut scalar_need = 0;
    loop {
        let count = match reader.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("原生工具候选扫描失败".into()),
            Ok(0) => break,
            Ok(count) => count,
        };
        for byte in &buffer[..count] {
            if *byte == b"Output:\n"[marker_match] {
                marker_match += 1;
                if marker_match == 8 {
                    marker = Some(None);
                    marker_match = 0;
                }
            } else {
                marker_match = usize::from(*byte == b'O');
            }
            offset += 1;
            let (character, start) = if scalar_len == 0 && byte.is_ascii() {
                (char::from(*byte), offset - 1)
            } else {
                if scalar_len == 0 {
                    scalar_need = match *byte {
                        0xc2..=0xdf => 2,
                        0xe0..=0xef => 3,
                        0xf0..=0xf4 => 4,
                        _ => return Err("原生工具候选 UTF-8 无效".into()),
                    };
                }
                scalar[scalar_len] = *byte;
                scalar_len += 1;
                if scalar_len != scalar_need {
                    continue;
                }
                let character = std::str::from_utf8(&scalar[..scalar_len])
                    .map_err(|_| "原生工具候选 UTF-8 无效")?
                    .chars()
                    .next()
                    .expect("nonempty scalar");
                let start = offset - scalar_len as u64;
                scalar_len = 0;
                (character, start)
            };
            if !character.is_whitespace() {
                first.get_or_insert(start);
                last = offset;
                if let Some(selected) = &mut marker {
                    selected.get_or_insert(start);
                }
                if line_head && character == '{' {
                    lines.push(start);
                }
                line_head = false;
            }
            if character == '\n' {
                line_head = true;
            }
        }
    }
    if scalar_len != 0 {
        return Err("原生工具候选 UTF-8 不完整".into());
    }
    let mut starts = Vec::new();
    for start in marker
        .flatten()
        .into_iter()
        .chain(first)
        .chain(lines.into_iter().rev())
    {
        if !starts.contains(&(start, last)) {
            starts.push((start, last));
        }
    }
    Ok(starts)
}

/// Only a single complete string candidate can be reopened with one origin.
/// Multiple giant text pieces cannot disappear by selecting just one of them.
pub(super) struct Candidate<'a> {
    pub span: &'a TextSpan,
    pub tool_error: Option<bool>,
}
pub(super) fn candidate(node: &Node) -> Result<Option<Candidate<'_>>, String> {
    match node {
        Node::String(Text::Span(span)) => Ok(Some(Candidate {
            span,
            tool_error: None,
        })),
        Node::Object(values) if !known(node) && mcp(node) => {
            let Some(mut selected) = candidate(values.get("content").expect("MCP content"))? else {
                return Ok(None);
            };
            selected.tool_error = Some(matches!(values.get("isError"), Some(Node::Bool(true))));
            Ok(Some(selected))
        }
        Node::Object(values)
            if !known(node)
                && (values.get("type").is_none()
                    || matches!(values.get("type"), Some(Node::String(Text::Inline(kind))) if matches!(kind.as_str(), "text" | "input_text" | "output_text" | "summary_text"))) =>
        {
            match values.get("text") {
                Some(Node::String(Text::Span(span))) => Ok(Some(Candidate {
                    span,
                    tool_error: None,
                })),
                _ => Ok(None),
            }
        }
        Node::Array(values) if values.len() == 1 => {
            let mut selected = candidate(&values[0])?;
            // Legacy Parser.output only takes the top-level MCP isError; an
            // array containing an MCP wrapper did not inherit that flag.
            if let Some(candidate) = &mut selected {
                candidate.tool_error = None;
            }
            Ok(selected)
        }
        _ => Ok(None),
    }
}

fn mcp(node: &Node) -> bool {
    let Node::Object(values) = node else {
        return false;
    };
    let Some(Node::Array(parts)) = values.get("content") else {
        return false;
    };
    if values.contains_key("type") {
        return false;
    }
    matches!(values.get("isError"), Some(Node::Bool(_))) || (!parts.is_empty() && parts.iter().all(|part| {
        let Node::Object(fields) = part else { return false };
        matches!(fields.get("type"), Some(Node::String(Text::Inline(kind))) if matches!(kind.as_str(), "text" | "input_text" | "output_text" | "summary_text" | "image" | "input_image" | "image_url"))
            || fields.contains_key("image_url")
    }))
}

#[cfg(test)]
mod tests;
