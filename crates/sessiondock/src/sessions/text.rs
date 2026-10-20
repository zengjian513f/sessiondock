//! Shared text, timestamp, and content-flattening primitives used by both
//! bounded row summaries (`sessions::index::summary`) and full projections
//! (`sessions::providers`).

use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use regex::Regex;
use serde_json::Value;

pub use crate::json_compat::{truthy, unquote};
use crate::sessions::providers::Skipped;

/// String form of a scalar JSON value that reaches a row field.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// `value` when truthy, as text.
pub fn text_if_truthy(value: &Value) -> Option<String> {
    truthy(value).then(|| py_str(value))
}

pub(in crate::sessions) fn string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}

pub(in crate::sessions) fn first_nonempty(values: &[Value], fallback: &str) -> String {
    values
        .iter()
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

/// Whitespace (Unicode White_Space plus the C0 separators 1C–1F).
pub fn py_is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

pub fn py_strip(text: &str) -> &str {
    text.trim_matches(py_is_space)
}

/// Line-split boundaries.
pub fn py_splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let boundary = match c {
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => Some(index + c.len_utf8()),
            '\r' => {
                let mut next = index + 1;
                if chars.peek().is_some_and(|(_, c)| *c == '\n') {
                    chars.next();
                    next += 1;
                }
                Some(next)
            }
            _ => None,
        };
        if let Some(next) = boundary {
            lines.push(&text[start..index]);
            start = next;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Whitespace-normalized, at most `n` characters plus `…`.
pub fn clip(text: &str, n: usize) -> String {
    let joined = text
        .split(py_is_space)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.chars().count() > n {
        joined.chars().take(n).collect::<String>() + "…"
    } else {
        joined
    }
}

pub(in crate::sessions) fn clip_title(text: &str) -> String {
    clip(text, 90)
}

/// A native timestamp as an RFC 3339 UTC instant with
/// millisecond precision, or `None` when it is falsy or unparsable.
pub fn norm_ts(value: &Value) -> Option<String> {
    if !truthy(value) {
        return None;
    }
    let instant = match value {
        Value::Number(number) => {
            let number = number.as_f64()?;
            let seconds = if number > 1e11 {
                number / 1000.0
            } else {
                number
            };
            let micros = (seconds * 1e6).round();
            if !micros.is_finite() || micros.abs() > i64::MAX as f64 {
                return None;
            }
            DateTime::from_timestamp_micros(micros as i64)?
        }
        Value::String(text) => parse_iso(&text.replace('Z', "+00:00"))?,
        _ => return None,
    };
    Some(instant.to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub(in crate::sessions) fn normalized(value: &Value) -> Value {
    let date = if let Some(number) = value.as_f64() {
        let seconds = if number > 1e11 {
            number / 1000.0
        } else {
            number
        };
        DateTime::from_timestamp_millis((seconds * 1000.0) as i64)
    } else if let Some(text) = value.as_str() {
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|dt| dt.to_utc())
    } else {
        None
    };
    date.map(|dt| Value::String(dt.to_rfc3339_opts(SecondsFormat::Millis, true)))
        .unwrap_or(Value::Null)
}

/// The subset of `datetime.fromisoformat` shapes native records use.
fn parse_iso(text: &str) -> Option<DateTime<Utc>> {
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M:%S%.f%z",
        "%Y-%m-%d %H:%M:%S%.f%z",
        "%Y-%m-%dT%H:%M%:z",
        "%Y-%m-%d %H:%M%:z",
    ] {
        if let Ok(parsed) = DateTime::parse_from_str(text, format) {
            return Some(parsed.to_utc());
        }
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(text, format) {
            return Some(parsed.and_utc());
        }
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|date| date.and_utc())
}

/// File time truncated to whole seconds.
pub fn iso_seconds(mtime_ns: u128) -> String {
    let seconds = i64::try_from(mtime_ns / 1_000_000_000).unwrap_or(i64::MAX);
    DateTime::from_timestamp(seconds, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

static INJECTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)AGENTS\.md instructions|<INSTRUCTIONS>|<user_info>|<environment_context>|<system-reminder>|<command-name>|Caveat: The messages below|<local-command-(?:caveat|stdout)>|<task-notification>|This session is being continued from a previous conversation|# Global User Guidance|<project_instructions>|<user_instructions>",
    )
    .expect("static regex")
});

static TIMELINE_PROTOCOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:#\s*AGENTS\.md instructions|#\s*Global User Guidance|<INSTRUCTIONS>|<user_info>|<environment_context>|<system-reminder>|<command-name>|<local-command-(?:caveat|stdout)>|<task-notification>|<project_instructions>|<user_instructions>|Caveat:\s*The messages below were generated by the user while running local commands|This session is being continued from a previous conversation)",
    )
    .expect("static regex")
});

static BASH_INPUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^\s*<bash-input>(.*?)</bash-input>\s*$").expect("static regex")
});

static PAIRED_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(command-[a-z-]+|system-reminder|local-command[a-z-]*)>").expect("static regex")
});

static SHORT_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]{1,40}>").expect("static regex"));

/// Grok wraps a user turn as `<image_files>…</image_files>` blocks followed by
/// `<user_query>…</user_query>`; images arrive as structured parts. A message
/// sent while Grok was working or after interrupting the previous turn also
/// carries a protocol prefix (`The user sent a message while you were
/// working:` / `The user interrupted the previous turn:`) and suffix (`Make
/// sure to complete any unfinished tasks from previous turns.`) outside the
/// tags; none of it is the user's text. Match the reference adapter:
/// whole-text, case-insensitive, and only one leading and one trailing
/// newline of the body are removed, not the user's indentation; any other
/// text outside the envelope keeps the record verbatim.
static GROK_USER_QUERY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?is)^\s*(?:<image_files>.*?</image_files>\s*)*",
        r"(?:The user (?:sent a message while you were working|interrupted the previous turn):\s*)?",
        r"(?:<image_files>.*?</image_files>\s*)*",
        r"<user_query>(.*?)</user_query>\s*",
        r"(?:Make sure to complete any unfinished tasks from previous turns\.)?\s*",
        r"(?:<skill_information>.*?</skill_information>\s*)?$",
    ))
    .unwrap()
});

pub(in crate::sessions) fn strip_grok_user_query(text: &str) -> String {
    let Some(captures) = GROK_USER_QUERY.captures(text) else {
        return text.to_owned();
    };
    let body = captures.get(1).map_or("", |m| m.as_str());
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .unwrap_or(body);
    body.strip_suffix("\r\n")
        .or_else(|| body.strip_suffix('\n'))
        .unwrap_or(body)
        .to_owned()
}

/// Claude's paste envelope repeats its id on the closing tag. Only unwrap a
/// complete matching pair; unknown/malformed markup and the pasted body stay
/// literal. This is display normalization, never a change to native input.
pub(in crate::sessions) fn claude_pasted_text(text: &str) -> String {
    static OPEN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<pasted_content id="([^"]+)">"#).unwrap());
    let mut out = String::with_capacity(text.len());
    let mut position = 0;
    while let Some(captures) = OPEN.captures_at(text, position) {
        let opening = captures.get(0).unwrap();
        let closing = format!("</pasted_content id=\"{}\">", &captures[1]);
        let Some(offset) = text[opening.end()..].find(&closing) else {
            out.push_str(&text[position..opening.end()]);
            position = opening.end();
            continue;
        };
        out.push_str(&text[position..opening.start()]);
        let body = &text[opening.end()..opening.end() + offset];
        let body = body
            .strip_prefix("\r\n")
            .or_else(|| body.strip_prefix('\n'))
            .unwrap_or(body);
        out.push_str(
            body.strip_suffix("\r\n")
                .or_else(|| body.strip_suffix('\n'))
                .unwrap_or(body),
        );
        position = opening.end() + offset + closing.len();
    }
    out.push_str(&text[position..]);
    out
}

/// Searches the first 2000 characters.
pub fn is_injected(text: &str) -> bool {
    let window: String = text.chars().take(2000).collect();
    INJECTED.is_match(&window)
}

/// A CLI protocol block at the very start.
pub fn is_timeline_protocol(text: &str) -> bool {
    TIMELINE_PROTOCOL.is_match(text)
}

/// True for `goal.internal_context` or a leading CLI protocol block.
pub fn is_codex_protocol_injection(text: &str, native_meta: &Value) -> bool {
    native_meta["content_item_kinds"]
        .as_array()
        .is_some_and(|kinds| kinds.iter().any(|kind| kind == "goal.internal_context"))
        || is_timeline_protocol(text)
        || crate::sessions::providers::envelopes::recommended_plugins(text)
}

pub(in crate::sessions) fn codex_internal(payload: &Value, text: &str) -> bool {
    payload["role"] == "developer"
        || (payload["role"] == "user"
            && is_codex_protocol_injection(
                text,
                &payload["internal_chat_message_metadata_passthrough"],
            ))
}

pub(in crate::sessions) fn strip_codex_abort_prefix(text: &str) -> String {
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if !lower.starts_with("<turn_aborted>") {
            break;
        }
        let Some(end) = lower.find("</turn_aborted>") else {
            break;
        };
        rest = trimmed[end + "</turn_aborted>".len()..].trim_start();
    }
    rest.to_owned()
}

pub(in crate::sessions) fn visible_text(content: &Value) -> String {
    // Inspect only native text blocks. Image source/file data is not title text;
    // ordinary user-authored JSON inside a text block remains ordinary text.
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.as_str() {
                    return Some(text);
                }
                match item["type"].as_str().unwrap_or("") {
                    "text" | "input_text" | "output_text" | "summary_text" => item["text"].as_str(),
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(_)
            if matches!(
                content["type"].as_str(),
                Some("text" | "input_text" | "output_text" | "summary_text")
            ) =>
        {
            content["text"].as_str().unwrap_or("").to_owned()
        }
        _ => String::new(),
    }
}

pub fn claude_bash_input(text: &str) -> Option<String> {
    let captures = BASH_INPUT.captures(text)?;
    let decoded = html_escape::decode_html_entities(captures.get(1).map_or("", |m| m.as_str()));
    let command = py_strip(&decoded);
    if command.is_empty() {
        return None;
    }
    Some(if command.starts_with('!') {
        command.to_owned()
    } else {
        format!("! {command}")
    })
}

/// Whether a match is returned: only an exact
/// sequence of `<bash-stdout>`/`<bash-stderr>` envelopes (case-insensitive,
/// paired with their own closing tag) and nothing but whitespace around them.
pub fn claude_bash_output_matches(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut spans = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let Some(relative) = lower[position..].find("<bash-std") else {
            break;
        };
        let start = position + relative;
        let stream = if lower[start..].starts_with("<bash-stdout>") {
            "stdout"
        } else if lower[start..].starts_with("<bash-stderr>") {
            "stderr"
        } else {
            position = start + 1;
            continue;
        };
        let body_start = start + "<bash-stdout>".len();
        let closing = format!("</bash-{stream}>");
        match lower[body_start..].find(&closing) {
            Some(offset) => {
                let end = body_start + offset + closing.len();
                spans.push((start, end));
                position = end;
            }
            None => position = start + 1,
        }
    }
    if spans.is_empty() {
        return false;
    }
    let mut remaining = String::new();
    let mut cursor = 0;
    for (start, end) in spans {
        remaining.push_str(&text[cursor..start]);
        cursor = end;
    }
    remaining.push_str(&text[cursor..]);
    py_strip(&remaining).is_empty()
}

fn strip_paired_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut position = 0;
    while let Some(captures) = PAIRED_TAG.captures_at(text, position) {
        let opening = captures.get(0).expect("whole match");
        let name = captures.get(1).map_or("", |m| m.as_str());
        let closing = format!("</{name}>");
        match text[opening.end()..].find(&closing) {
            Some(offset) => {
                out.push_str(&text[position..opening.start()]);
                out.push(' ');
                position = opening.end() + offset + closing.len();
            }
            None => {
                let step = text[opening.start()..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
                out.push_str(&text[position..opening.start() + step]);
                position = opening.start() + step;
            }
        }
    }
    out.push_str(&text[position..]);
    out
}

pub fn title_from_text(text: &str) -> String {
    let text = strip_paired_tags(text);
    let text = SHORT_TAG.replace_all(&text, " ");
    for line in py_splitlines(&text) {
        let line = py_strip(line);
        if line.chars().count() >= 4
            && !(line.starts_with('#')
                || line.starts_with('-')
                || line.starts_with('*')
                || line.starts_with("```"))
        {
            return clip(line, 90);
        }
    }
    let fallback = clip(&text, 90);
    if fallback.is_empty() {
        "(无标题)".to_owned()
    } else {
        fallback
    }
}

/// Restricted to `kind == "text"` parts, joined
/// with newlines. Shapes the reference adapter cannot read either (a scalar
/// `content`, a non-string `text`) are the same hard failures as today.
pub fn flatten_text(content: &Value) -> Result<String, String> {
    let mut parts = Vec::new();
    match content {
        Value::Null => {}
        Value::String(text) => parts.push(text.clone()),
        Value::Object(_) => push_text_block(content, &mut parts)?,
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(text) => parts.push(text.clone()),
                    Value::Object(_) => push_text_block(item, &mut parts)?,
                    _ => {}
                }
            }
        }
        _ => return Err("原生 content 类型无效".to_owned()),
    }
    Ok(parts.join("\n"))
}

fn push_text_block(block: &Value, parts: &mut Vec<String>) -> Result<(), String> {
    if matches!(
        block["type"].as_str(),
        Some("text" | "input_text" | "output_text" | "summary_text")
    ) {
        match &block["text"] {
            Value::Null => parts.push(String::new()),
            Value::String(text) => parts.push(text.clone()),
            _ => return Err("原生文本块的 text 必须是字符串".to_owned()),
        }
    }
    Ok(())
}

/// Exit code over the first 400 characters, case-insensitively:
/// `"exit_code"\s*:\s*(-?\d+)` or `\bexit(?:ed)?(?: with)?(?: code| status)? (-?\d+)`
/// — the plain form needs exactly one space before the number, so `exit: 1`
/// (Grok's output header) is not a code.
pub(in crate::sessions) fn output_exit_code(text: &str) -> Option<i64> {
    fn integer(value: &str) -> Option<i64> {
        let end = value
            .char_indices()
            .take_while(|(index, ch)| ch.is_ascii_digit() || (*index == 0 && *ch == '-'))
            .map(|(index, ch)| index + ch.len_utf8())
            .last()?;
        value[..end].parse().ok()
    }
    let prefix = text.chars().take(400).collect::<String>().to_lowercase();
    if let Some((_, rest)) = prefix.split_once("\"exit_code\"")
        && let Some(rest) = rest.trim_start().strip_prefix(':')
        && let Some(code) = integer(rest.trim_start())
    {
        return Some(code);
    }
    for (index, _) in prefix.match_indices("exit") {
        if prefix[..index]
            .chars()
            .last()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        {
            continue;
        }
        let mut rest = &prefix[index + 4..];
        if let Some(tail) = rest.strip_prefix("ed") {
            rest = tail
        }
        if let Some(tail) = rest.strip_prefix(" with") {
            rest = tail
        }
        if let Some(tail) = rest
            .strip_prefix(" code")
            .or_else(|| rest.strip_prefix(" status"))
        {
            rest = tail
        }
        if let Some(tail) = rest.strip_prefix(' ')
            && let Some(code) = integer(tail)
        {
            return Some(code);
        }
    }
    None
}

pub(in crate::sessions) fn is_question(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "askuserquestion"
        || lower == "request_user_input"
        || lower.ends_with(".request_user_input")
        || lower.ends_with("__request_user_input")
}

/// Claude/Grok tool results drop empty text blocks before joining, exactly
/// like the reference adapter; ordinary message content keeps them.
pub(in crate::sessions) fn tool_text_parts(
    content: &Value,
    skipped: &mut Skipped,
) -> Result<String, String> {
    match content {
        Value::Array(items) => Ok(items
            .iter()
            .map(|item| text_part(item, skipped))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n")),
        other => text_parts(other, skipped),
    }
}

/// Text of a content value. Blocks of an unknown non-image kind (and
/// non-string/non-object array elements) are skipped and counted, like the
/// reference `_flatten_content`; a text block whose `text` is not a string and
/// a top-level scalar remain hard failures (the reference adapter raises).
pub(in crate::sessions) fn text_parts(
    content: &Value,
    skipped: &mut Skipped,
) -> Result<String, String> {
    Ok(text_part(content, skipped)?.unwrap_or_default())
}

/// `Ok(None)` is a skipped block: it contributes no line to a joined array.
fn text_part(content: &Value, skipped: &mut Skipped) -> Result<Option<String>, String> {
    match content {
        Value::Null => Ok(Some(String::new())),
        Value::String(text) => Ok(Some(text.clone())),
        Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                if item.is_string() || item.is_object() || item.is_array() {
                    if let Some(part) = text_part(item, skipped)? {
                        parts.push(part);
                    }
                } else {
                    skipped.note("内容块类型", "(非对象元素)");
                }
            }
            Ok(Some(parts.join("\n")))
        }
        Value::Object(_) => match content["type"].as_str().unwrap_or("") {
            "text" | "input_text" | "output_text" | "summary_text" => match &content["text"] {
                Value::Null => Ok(Some(String::new())),
                Value::String(text) => Ok(Some(text.clone())),
                _ => Err("原生文本块的 text 必须是字符串".into()),
            },
            other => {
                skipped.note("内容块类型", other);
                Ok(None)
            }
        },
        _ => Err("原生 content 类型无效".to_owned()),
    }
}
