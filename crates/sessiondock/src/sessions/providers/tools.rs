//! Pure presentation of known tool arguments. A shell command is text here:
//! these helpers never run commands or read paths mentioned in a tool call.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use super::{string, truthy};

const DIFF_BYTES: usize = 512 * 1024;
const DIFF_LINES: usize = 20_000;
const DIFF_WORK: usize = 4_000_000;

static SHELL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:/usr/bin/|/bin/)?(?:ba|z|da)?sh\s+(?:-[A-Za-z]+\s+)*").unwrap()
});
static COMMAND_OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-[A-Za-z]*c$").unwrap());
static EXEC_COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bcmd["']?\s*:\s*("(?:\\.|[^"\\])*")"#).unwrap());
static EXEC_STEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bstep["']?\s*:\s*"((?:\\.|[^"\\])*)""#).unwrap());
static EXEC_TOOL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\btools\.(\w+)\s*\(").unwrap());
static STRING_LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)"(?:\\.|[^"\\])*""#).unwrap());
static CANCELLED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^aborted by user(?:\s|$)").unwrap());

fn decoded(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| value.clone())
}

fn key(name: &str) -> String {
    name.to_lowercase()
        .rsplit("__")
        .next()
        .unwrap_or("")
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn clip(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > limit {
        text.chars().take(limit).collect::<String>() + "…"
    } else {
        text
    }
}

fn first<'a>(data: &'a Value, fields: &[&str]) -> &'a Value {
    fields
        .iter()
        .map(|field| &data[*field])
        .find(|value| truthy(value))
        .unwrap_or(&Value::Null)
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Null => "None".to_owned(),
        _ => string(value),
    }
}

fn shell_command(value: &Value) -> String {
    let command = if let Some(items) = value.as_array() {
        if items.len() >= 3 && COMMAND_OPTION.is_match(&scalar(&items[items.len() - 2])) {
            scalar(items.last().unwrap())
        } else {
            items.iter().map(scalar).collect::<Vec<_>>().join(" ")
        }
    } else {
        scalar(value)
    };
    let raw = command.trim();
    let stripped = SHELL.replace(raw, "");
    let mut shown = stripped.as_ref();
    if shown != raw
        && shown.len() >= 2
        && ((shown.starts_with('\'') && shown.ends_with('\''))
            || (shown.starts_with('"') && shown.ends_with('"')))
    {
        shown = &shown[1..shown.len() - 1]
    }
    clip(if shown.is_empty() { raw } else { shown }, usize::MAX)
}

pub(super) fn summary(name: &str, value: &Value) -> Option<String> {
    let data = decoded(value);
    let key = key(name);
    if [
        "bash",
        "shell",
        "local_shell_call",
        "exec_command",
        "terminal",
        "run_terminal_cmd",
    ]
    .contains(&key.as_str())
    {
        let command = if data.is_object() {
            let command = first(&data, &["command", "cmd"]);
            if truthy(command) {
                command
            } else {
                &data["action"]["command"]
            }
        } else {
            &data
        };
        if truthy(command) {
            return Some(clip(&format!("$ {}", shell_command(command)), 200));
        }
    }
    if key == "exec"
        && let Some(code) = data.as_str()
    {
        let commands = EXEC_COMMAND
            .captures_iter(code)
            .filter_map(|matched| serde_json::from_str::<Value>(&matched[1]).ok())
            .map(|command| shell_command(&command))
            .collect::<Vec<_>>();
        if let Some(command) = commands.first() {
            let suffix = if commands.len() > 1 {
                format!(" …(+{})", commands.len() - 1)
            } else {
                String::new()
            };
            return Some(clip(&format!("$ {command}"), 200) + &suffix);
        }
        let steps = EXEC_STEP
            .captures_iter(code)
            .map(|matched| matched[1].to_owned())
            .collect::<Vec<_>>();
        let mut called = Vec::new();
        for matched in EXEC_TOOL.captures_iter(code) {
            if !called.iter().any(|item| item == &matched[1]) {
                called.push(matched[1].to_owned())
            }
        }
        if called.iter().any(|item| item == "update_plan") && !steps.is_empty() {
            return Some(clip(
                &format!(
                    "计划 ×{}: {}",
                    steps.len(),
                    steps.iter().take(2).cloned().collect::<Vec<_>>().join("; ")
                ),
                200,
            ));
        }
        if !called.is_empty() {
            return Some(clip(
                &format!(
                    "tools.{}",
                    called
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("  tools.")
                ),
                200,
            ));
        }
    }
    let object = data.as_object()?;
    let path = first(&data, &["file_path", "path", "target_file"]);
    if ["read", "read_file", "notebookread", "open"].contains(&key.as_str()) && truthy(path) {
        let span = if truthy(&data["offset"]) || truthy(&data["limit"]) {
            let offset = if truthy(&data["offset"]) {
                scalar(&data["offset"])
            } else {
                "0".to_owned()
            };
            let limit = if truthy(&data["limit"]) {
                scalar(&data["limit"])
            } else {
                String::new()
            };
            format!(" ⌖{offset}+{limit}")
                .trim_end_matches('+')
                .to_owned()
        } else {
            String::new()
        };
        return Some(clip(&format!("读 {}{span}", scalar(path)), 200));
    }
    if ["grep", "grep_search", "search", "rg", "codebase_search"].contains(&key.as_str()) {
        let pattern = first(&data, &["pattern", "query", "regex"]);
        let scope = first(&data, &["path", "glob", "include"]);
        if truthy(pattern) {
            return Some(clip(
                &format!(
                    "搜 {}{}",
                    scalar(pattern),
                    if truthy(scope) {
                        format!(" ⌁ {}", scalar(scope))
                    } else {
                        String::new()
                    }
                ),
                200,
            ));
        }
    }
    if ["glob", "find", "list_dir", "ls", "file_search"].contains(&key.as_str()) {
        let target = if truthy(&data["pattern"]) {
            &data["pattern"]
        } else {
            path
        };
        if truthy(target) {
            return Some(clip(&format!("找 {}", scalar(target)), 200));
        }
    }
    if ["webfetch", "web_fetch", "fetch"].contains(&key.as_str()) && truthy(&data["url"]) {
        return Some(clip(&format!("抓 {}", scalar(&data["url"])), 200));
    }
    if ["websearch", "web_search"].contains(&key.as_str()) && truthy(&data["query"]) {
        return Some(clip(&format!("搜索 {}", scalar(&data["query"])), 200));
    }
    if ["task", "agent"].contains(&key.as_str()) {
        let title = first(&data, &["description", "prompt"]);
        let kind = first(&data, &["subagent_type", "agentType"]);
        if truthy(title) {
            return Some(clip(
                &format!(
                    "子代理{}: {}",
                    if truthy(kind) {
                        format!("({})", scalar(kind))
                    } else {
                        String::new()
                    },
                    scalar(title)
                ),
                200,
            ));
        }
    }
    if key == "todowrite" || key == "update_plan" {
        let (field, label, max) = if key == "todowrite" {
            ("todos", "TODO", 3)
        } else {
            ("plan", "计划", 2)
        };
        let entries = data[field]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter(|item| item.is_object())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !entries.is_empty() {
            let shown = entries
                .iter()
                .take(max)
                .map(|entry| {
                    let value = if key == "todowrite" {
                        first(entry, &["subject", "content"])
                    } else {
                        &entry["step"]
                    };
                    clip(
                        &if truthy(value) {
                            scalar(value)
                        } else {
                            String::new()
                        },
                        36,
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Some(clip(&format!("{label} ×{}: {shown}", entries.len()), 200));
        }
    }
    let values = object
        .iter()
        .filter(|(_, value)| value.is_string() || value.is_number() || value.is_boolean())
        .filter(|(_, value)| !scalar(value).trim().is_empty())
        .take(4)
        .map(|(key, value)| format!("{key}={}", scalar(value)))
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| clip(&values.join("  "), 200))
}

pub(super) fn question(name: &str, value: &Value) -> Option<(String, Vec<Value>)> {
    if !super::is_question(name) {
        return None;
    }
    let data = decoded(value);
    let rows = if data["questions"].is_object() {
        vec![&data["questions"]]
    } else {
        data["questions"].as_array()?.iter().collect()
    };
    let mut questions = Vec::new();
    for row in rows {
        if !row.is_object() || !truthy(&row["question"]) {
            continue;
        }
        let options = row["options"].as_array().into_iter().flatten().filter_map(|option| {
            if option.is_string() { Some(json!({"label": option, "description": ""})) }
            else if option.is_object() && truthy(&option["label"]) {
                Some(json!({"label": scalar(&option["label"]), "description": if truthy(&option["description"]) { scalar(&option["description"]) } else { String::new() }}))
            } else { None }
        }).collect::<Vec<_>>();
        questions.push(json!({
            "header": if truthy(&row["header"]) { scalar(&row["header"]) } else { String::new() },
            "question": scalar(&row["question"]), "options": options,
            "multiple": truthy(&row["multiSelect"]) || truthy(&row["multiple"]),
        }));
    }
    if questions.is_empty() {
        return None;
    }
    Some((
        questions
            .iter()
            .filter_map(|question| question["question"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
        questions,
    ))
}

/// Python `_stringify` recursively joins arrays, takes a block's `text`, and
/// serializes other objects. Image-shaped blocks retain the visible placeholder.
fn output_text(value: &Value) -> Result<String, String> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(text) => Ok(text.clone()),
        Value::Array(items) => items
            .iter()
            .map(output_text)
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| parts.join("\n")),
        Value::Object(_) => {
            if super::image_content::image_shape(value) {
                return Ok("[图片]".to_owned());
            }
            if super::image_content::tool_wrapper(value) {
                return output_text(&value["content"]);
            }
            if let Some(text) = value.get("text") {
                output_text(text)
            } else {
                Ok(string(value))
            }
        }
        _ => Ok(scalar(value)),
    }
}

fn is_output_envelope(value: &Value) -> bool {
    value.is_object()
        && value.get("output").is_some()
        && value.get("wall_time_seconds").is_some()
        && ["exit_code", "session_id", "chunk_id"]
            .iter()
            .any(|key| value.get(key).is_some())
}

/// One streamed chunk of a multi-part Codex tool output: a part that is by
/// itself a complete envelope. A structural chunk is the original array
/// element (a giant part already decoded by the native replay, whose nested
/// media keeps its span authority); a parsed chunk came out of a small text
/// part and owns a fresh tree with no such authority.
enum Chunk<'a> {
    Structural(&'a Value),
    Parsed(Value),
}
impl Chunk<'_> {
    fn envelope(&self) -> &Value {
        match self {
            Chunk::Structural(value) => value,
            Chunk::Parsed(value) => value,
        }
    }
}

/// Codex records the result of an `exec` script as `[header, chunk, chunk, …]`
/// where the header is `Script completed\nWall time …\nOutput:\n` and every
/// chunk is one stringified envelope (`{"chunk_id","wall_time_seconds",
/// "exit_code"?,"session_id"?,"output"}`); a trailing empty part or an image
/// part may follow and the script's own prints (`--- 1 ---`, `{}`, status
/// objects) may sit between chunks. Every chunk's `output` is shown, in part
/// order, so no streamed output is lost. Only whole parts qualify: a part is a
/// chunk when its Unicode-trimmed text is exactly one envelope object (or it is
/// a decoded structural envelope); the header, prints and prefixed forms are
/// not chunks and the single-envelope search below still handles them.
/// `Ok(None)` when no part is a chunk.
fn chunk_envelopes(value: &Value) -> Result<Option<Vec<(usize, Chunk<'_>)>>, String> {
    let Some(parts) = value.as_array() else {
        return Ok(None);
    };
    let mut chunks = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if is_output_envelope(part) {
            chunks.push((index, Chunk::Structural(part)));
            continue;
        }
        let Some(text) = part.as_str().or_else(|| part["text"].as_str()) else {
            continue;
        };
        let text = text.trim();
        if !(text.starts_with('{') && text.ends_with('}'))
            || !text.contains("\"wall_time_seconds\"")
            || !text.contains("\"output\"")
        {
            continue;
        }
        if let Ok(envelope) = serde_json::from_str::<Value>(text)
            && is_output_envelope(&envelope)
        {
            chunks.push((index, Chunk::Parsed(envelope)));
        }
    }
    Ok((!chunks.is_empty()).then_some(chunks))
}

/// Only this known Codex tool envelope establishes a JSON-string structure
/// boundary. Ordinary content strings and JSON tutorials are never decoded.
/// Search all source candidates without block/line/aggregate-byte quotas.
/// Candidate offsets borrow the original text rather than copying suffixes.
fn output_envelope(value: &Value, raw: &str) -> Result<Option<Value>, String> {
    if is_output_envelope(value) {
        return Ok(Some(value.clone()));
    }
    if !raw.contains("\"wall_time_seconds\"") || !raw.contains("\"output\"") {
        return Ok(None);
    }
    let mut candidates = vec![raw];
    if let Some(parts) = value.as_array() {
        let texts = parts
            .iter()
            .filter_map(|part| part.as_str().or_else(|| part["text"].as_str()));
        candidates.extend(texts);
    } else if let Some(text) = value["text"].as_str() {
        candidates.push(text);
    }
    for text in candidates {
        if !text.contains("\"wall_time_seconds\"") || !text.contains("\"output\"") {
            continue;
        }
        let marker = text.rfind("Output:\n").map(|index| index + 8);
        let mut starts = Vec::new();
        if let Some(marker) = marker {
            starts.push(marker);
        }
        starts.push(0);
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            // Scan each line once, never trim/parse every growing suffix.
            if line.trim_start().starts_with('{') {
                starts.push(offset);
            }
            offset += line.len();
        }
        let mut seen = std::collections::HashSet::new();
        for start in starts {
            if !seen.insert(start) {
                continue;
            }
            let candidate = &text[start..];
            let candidate = candidate.trim();
            if !candidate.starts_with('{') {
                continue;
            }
            let Ok(envelope) = serde_json::from_str::<Value>(candidate) else {
                continue;
            };
            if is_output_envelope(&envelope) {
                return Ok(Some(envelope));
            }
        }
    }
    Ok(None)
}

pub(super) fn sanitize_output_with_media(
    value: &Value,
    context: Option<&super::MediaContext<'_>>,
) -> Result<(Value, Vec<crate::media::NativeImage>), String> {
    fn sanitize(
        value: &Value,
        context: Option<&super::MediaContext<'_>>,
    ) -> Result<(Value, Vec<crate::media::NativeImage>), String> {
        // Extract from the ORIGINAL borrowed structural envelope before clone.
        if is_output_envelope(value) {
            let (output, media) = sanitize(&value["output"], context)?;
            let mut envelope = value.clone();
            envelope["output"] = output;
            return Ok((envelope, media));
        }
        // Multi-part output: the cleaned value is the array of its chunk
        // envelopes only (header/prints dropped); image parts next to the
        // chunks and media inside a structural chunk keep their authority
        // because both are looked up on the original elements.
        if let Some(chunks) = chunk_envelopes(value)? {
            let mut media = Vec::new();
            let mut parts = Vec::with_capacity(chunks.len());
            let mut chunks = chunks.into_iter().peekable();
            for (index, part) in value.as_array().into_iter().flatten().enumerate() {
                if chunks.peek().is_some_and(|(at, _)| *at == index) {
                    // Structural chunks are sanitized at their original
                    // address; parsed JSON strings have different trees and
                    // no span authority.
                    let (output, mut nested, mut envelope) = match chunks.next().unwrap().1 {
                        Chunk::Structural(value) => {
                            let (output, nested) = sanitize(&value["output"], context)?;
                            (output, nested, value.clone())
                        }
                        Chunk::Parsed(value) => {
                            let (output, nested) = sanitize(&value["output"], None)?;
                            (output, nested, value)
                        }
                    };
                    media.append(&mut nested);
                    envelope["output"] = output;
                    parts.push(envelope);
                    continue;
                }
                // Text blocks need no copy because they never enter the
                // cleaned chunk array. Other serializable parts follow
                // Python `_stringify` rather than rejecting the tool result.
                let text_block = matches!(
                    part["type"].as_str(),
                    Some("text" | "input_text" | "output_text" | "summary_text")
                );
                if text_block {
                    continue;
                }
                if let Some(image) = super::image_content::native_image(part, context)? {
                    media.push(image);
                } else {
                    output_text(part)?;
                }
            }
            return Ok((Value::Array(parts), media));
        }
        let (cleaned, mut media) = super::image_content::sanitize_tool_with_media(value, context)?;
        let raw = output_text(&cleaned)?;
        if let Some(mut envelope) = output_envelope(&cleaned, &raw)? {
            // Parsed JSON strings have different trees and no span authority.
            let (output, mut nested) = sanitize(&envelope["output"], None)?;
            media.append(&mut nested);
            envelope["output"] = output;
            return Ok((envelope, media));
        }
        Ok((cleaned, media))
    }
    sanitize(value, context)
}

pub(super) fn output(value: &Value) -> Result<(String, Value), String> {
    if let Some(chunks) = chunk_envelopes(value)? {
        // Validate that every other part can be rendered using the same
        // Python-compatible fallback; text blocks need no copy here.
        for part in value.as_array().into_iter().flatten() {
            if !is_output_envelope(part)
                && !matches!(
                    part["type"].as_str(),
                    Some("text" | "input_text" | "output_text" | "summary_text")
                )
            {
                output_text(part)?;
            }
        }
        return chunked(chunks.iter().map(|(_, chunk)| chunk));
    }
    let raw = output_text(value)?;
    if let Some(envelope) = output_envelope(value, &raw)? {
        return chunked(std::iter::once(&Chunk::Parsed(envelope)));
    }
    Ok((raw, json!({})))
}

/// Text and fields of one or more envelopes in part order: the outputs are
/// concatenated as recorded (no separator — each chunk carries its own
/// newlines), `exit_code` is the last chunk's, `duration_s` sums every
/// chunk's `wall_time_seconds`, and an MCP `isError` in any chunk marks the
/// result. The reference adapter shows a single chunk instead (documented
/// superset, docs/migration.md).
fn chunked<'a>(chunks: impl Iterator<Item = &'a Chunk<'a>>) -> Result<(String, Value), String> {
    let mut text = String::new();
    let (mut exit_code, mut duration, mut error) = (None, None, false);
    for envelope in chunks.map(Chunk::envelope) {
        text.push_str(&output_text(&envelope["output"])?);
        exit_code = envelope["exit_code"].as_i64().or(exit_code);
        if let Some(seconds) = envelope["wall_time_seconds"].as_f64() {
            duration = Some(duration.unwrap_or(0.0) + seconds);
        }
        error |= super::image_content::tool_wrapper(&envelope["output"])
            && envelope["output"]["isError"] == true;
    }
    let mut fields = json!({});
    if let Some(code) = exit_code {
        fields["exit_code"] = json!(code);
    }
    if let Some(seconds) = duration {
        fields["duration_s"] = json!(seconds);
    }
    if error {
        fields["error"] = json!(true);
    }
    Ok((text, fields))
}

pub(super) fn answer(value: &Value) -> Result<(String, bool), String> {
    let text = output_text(value)?;
    if CANCELLED.is_match(text.trim()) {
        return Ok(("已取消回答".to_owned(), true));
    }
    let data = decoded(value);
    let Some(answers) = data["answers"].as_object() else {
        return Ok((text, false));
    };
    let rows = answers
        .values()
        .filter_map(|answer| {
            let values = if answer.is_object() {
                &answer["answers"]
            } else {
                answer
            };
            let shown = if let Some(items) = values.as_array() {
                items
                    .iter()
                    .map(scalar)
                    .filter(|item| !item.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("、")
            } else if truthy(values) {
                scalar(values).trim().to_owned()
            } else {
                String::new()
            };
            (!shown.is_empty()).then_some(shown)
        })
        .collect::<Vec<_>>();
    Ok((
        if rows.is_empty() {
            text
        } else {
            rows.join("\n")
        },
        false,
    ))
}

fn quoted_patch(text: &str) -> Option<String> {
    if text.trim_start().starts_with("*** Begin Patch") {
        return Some(text.trim_start().to_owned());
    }
    STRING_LITERAL
        .find_iter(text)
        .filter_map(|matched| serde_json::from_str::<String>(matched.as_str()).ok())
        .find(|value| value.trim_start().starts_with("*** Begin Patch"))
        .map(|patch| patch.trim_start().to_owned())
}

fn lines(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut from = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if matches!(
            ch,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            result.push(&text[from..index]);
            from = index + ch.len_utf8();
            if ch == '\r' && chars.peek().is_some_and(|(_, ch)| *ch == '\n') {
                from = chars.next().unwrap().0 + 1;
            }
        }
    }
    if from < text.len() {
        result.push(&text[from..])
    }
    result
}

fn bounded(texts: &[&str]) -> Result<(), String> {
    if texts.iter().map(|text| text.len()).sum::<usize>() > DIFF_BYTES {
        return Err("文件修改展示超过 512 KiB 预算；完整原始工具参数仍保留在 text".to_owned());
    }
    if texts.iter().map(|text| lines(text).len()).sum::<usize>() > DIFF_LINES {
        return Err("文件修改展示超过 20000 行预算；完整原始工具参数仍保留在 text".to_owned());
    }
    Ok(())
}

pub(super) fn changes(name: &str, value: &Value) -> Result<Vec<Value>, String> {
    let data = decoded(value);
    let key = key(name);
    let patch = if data.is_object() {
        ["patch", "input"]
            .iter()
            .filter_map(|field| data[*field].as_str())
            .find_map(quoted_patch)
    } else {
        data.as_str().and_then(quoted_patch)
    };
    if let Some(patch) = patch {
        bounded(&[&patch])?;
        return Ok(patch_changes(&patch));
    }
    if !data.is_object() {
        return Ok(Vec::new());
    }
    let path = first(&data, &["file_path", "path"]).as_str().unwrap_or("");
    if path.is_empty() {
        return Ok(Vec::new());
    }
    if ["edit", "str_replace"].contains(&key.as_str())
        && data.get("old_string").is_some()
        && data.get("new_string").is_some()
    {
        return Ok(vec![edit_change(
            path,
            &data["old_string"],
            &data["new_string"],
        )?]);
    }
    if ["multiedit", "multi_edit"].contains(&key.as_str()) {
        let mut changes = Vec::new();
        let mut total_bytes = 0;
        let mut total_lines = 0;
        for edit in data["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|edit| edit.is_object())
        {
            for field in ["old_string", "new_string"] {
                let text = if truthy(&edit[field]) {
                    scalar(&edit[field])
                } else {
                    String::new()
                };
                total_bytes += text.len();
                total_lines += lines(&text).len();
            }
            if total_bytes > DIFF_BYTES || total_lines > DIFF_LINES {
                return Err(
                    "MultiEdit 总展示超过 512 KiB / 20000 行预算；完整原始工具参数仍保留在 text"
                        .to_owned(),
                );
            }
            changes.push(edit_change(path, &edit["old_string"], &edit["new_string"])?);
        }
        return Ok(changes);
    }
    if ["write", "write_file"].contains(&key.as_str()) && data.get("content").is_some() {
        let content = if truthy(&data["content"]) {
            scalar(&data["content"])
        } else {
            String::new()
        };
        bounded(&[&content])?;
        let rows = lines(&content);
        return Ok(vec![json!({
            "path": path, "operation": "write", "patch": rows.iter().map(|line| format!("+{line}")).collect::<Vec<_>>().join("\n"),
            "added": rows.len(), "removed": 0, "before_available": false, "after_available": true,
            "before_complete": false, "after_complete": true,
        })]);
    }
    Ok(Vec::new())
}

fn patch_changes(patch: &str) -> Vec<Value> {
    fn finish(current: &mut Option<Value>, body: &mut Vec<String>, out: &mut Vec<Value>) {
        if let Some(mut change) = current.take() {
            change["patch"] = json!(body.join("\n"));
            change["added"] = json!(
                body.iter()
                    .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
                    .count()
            );
            change["removed"] = json!(
                body.iter()
                    .filter(|line| line.starts_with('-') && !line.starts_with("---"))
                    .count()
            );
            out.push(change);
        }
        body.clear();
    }
    let mut current = None;
    let mut body = Vec::new();
    let mut out = Vec::new();
    for line in lines(patch) {
        let header = ["Add", "Update", "Delete"].iter().find_map(|operation| {
            line.strip_prefix(&format!("*** {operation} File: "))
                .filter(|path| !path.is_empty())
                .map(|path| (*operation, path.trim()))
        });
        if let Some((operation, path)) = header {
            finish(&mut current, &mut body, &mut out);
            current = Some(json!({
                "path": path, "operation": operation.to_lowercase(),
                "before_complete": operation == "Delete", "after_complete": operation == "Add",
                "before_available": operation != "Add", "after_available": operation != "Delete",
            }));
            continue;
        }
        if let Some(current) = &mut current {
            if ["*** Begin Patch", "*** End Patch"].contains(&line) {
                continue;
            }
            if let Some(path) = line.strip_prefix("*** Move to: ") {
                current["new_path"] = json!(path.trim())
            } else {
                body.push(line.to_owned())
            }
        }
    }
    finish(&mut current, &mut body, &mut out);
    out
}

#[derive(Clone, Copy, Debug)]
struct Code {
    tag: &'static str,
    a: usize,
    ae: usize,
    b: usize,
    be: usize,
}

fn matching_blocks(a: &[&str], b: &[&str]) -> Result<Vec<(usize, usize, usize)>, String> {
    fn spend(work: &mut usize) -> Result<(), String> {
        *work = work.checked_sub(1).ok_or_else(|| {
            "文件差分匹配超过 4000000 步预算；完整原始工具参数仍保留在 text".to_owned()
        })?;
        Ok(())
    }
    let mut positions = HashMap::<&str, Vec<usize>>::new();
    for (index, line) in b.iter().enumerate() {
        positions.entry(line).or_default().push(index)
    }
    if b.len() >= 200 {
        positions.retain(|_, indices| indices.len() <= b.len() / 100 + 1)
    }
    let mut work = DIFF_WORK;
    let mut pending = vec![(0, a.len(), 0, b.len())];
    let mut blocks = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = pending.pop() {
        let (mut best_a, mut best_b, mut size) = (alo, blo, 0);
        let mut lengths = HashMap::<usize, usize>::new();
        for (i, line) in a.iter().enumerate().take(ahi).skip(alo) {
            spend(&mut work)?;
            let mut next = HashMap::new();
            if let Some(indices) = positions.get(line) {
                for &j in indices {
                    spend(&mut work)?;
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let length = if j == 0 {
                        1
                    } else {
                        lengths.get(&(j - 1)).copied().unwrap_or(0) + 1
                    };
                    next.insert(j, length);
                    if length > size {
                        (best_a, best_b, size) = (i + 1 - length, j + 1 - length, length)
                    }
                }
            }
            lengths = next;
        }
        while best_a > alo && best_b > blo && a[best_a - 1] == b[best_b - 1] {
            spend(&mut work)?;
            best_a -= 1;
            best_b -= 1;
            size += 1
        }
        while best_a + size < ahi && best_b + size < bhi && a[best_a + size] == b[best_b + size] {
            spend(&mut work)?;
            size += 1
        }
        if size > 0 {
            blocks.push((best_a, best_b, size));
            if alo < best_a && blo < best_b {
                pending.push((alo, best_a, blo, best_b))
            }
            if best_a + size < ahi && best_b + size < bhi {
                pending.push((best_a + size, ahi, best_b + size, bhi))
            }
        }
    }
    blocks.sort_unstable();
    let mut merged: Vec<(usize, usize, usize)> = Vec::new();
    for (a, b, size) in blocks {
        if let Some(last) = merged.last_mut()
            && last.0 + last.2 == a
            && last.1 + last.2 == b
        {
            last.2 += size
        } else {
            merged.push((a, b, size))
        }
    }
    merged.push((a.len(), b.len(), 0));
    Ok(merged)
}

fn unified(path: &str, a: &[&str], b: &[&str]) -> Result<Vec<String>, String> {
    let blocks = matching_blocks(a, b)?;
    let (mut i, mut j) = (0, 0);
    let mut codes = Vec::new();
    for (ai, bj, size) in blocks {
        let tag = match (i < ai, j < bj) {
            (true, true) => "replace",
            (true, false) => "delete",
            (false, true) => "insert",
            _ => "",
        };
        if !tag.is_empty() {
            codes.push(Code {
                tag,
                a: i,
                ae: ai,
                b: j,
                be: bj,
            })
        }
        i = ai + size;
        j = bj + size;
        if size > 0 {
            codes.push(Code {
                tag: "equal",
                a: ai,
                ae: i,
                b: bj,
                be: j,
            })
        }
    }
    if codes.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(first) = codes.first_mut()
        && first.tag == "equal"
    {
        first.a = first.a.max(first.ae.saturating_sub(3));
        first.b = first.b.max(first.be.saturating_sub(3));
    }
    if let Some(last) = codes.last_mut()
        && last.tag == "equal"
    {
        last.ae = last.ae.min(last.a + 3);
        last.be = last.be.min(last.b + 3);
    }
    let mut groups = Vec::new();
    let mut group = Vec::new();
    for mut code in codes {
        if code.tag == "equal" && code.ae - code.a > 6 {
            group.push(Code {
                ae: code.a + 3,
                be: code.b + 3,
                ..code
            });
            groups.push(group);
            group = Vec::new();
            code.a = code.ae - 3;
            code.b = code.be - 3;
        }
        group.push(code);
    }
    if !group.is_empty() && !(group.len() == 1 && group[0].tag == "equal") {
        groups.push(group)
    }
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    fn range(start: usize, end: usize) -> String {
        match end - start {
            0 => format!("{start},0"),
            1 => (start + 1).to_string(),
            length => format!("{},{length}", start + 1),
        }
    }
    let mut out = vec![format!("--- {path}"), format!("+++ {path}")];
    for group in groups {
        let first = group.first().unwrap();
        let last = group.last().unwrap();
        out.push(format!(
            "@@ -{} +{} @@",
            range(first.a, last.ae),
            range(first.b, last.be)
        ));
        for code in group {
            if code.tag == "equal" {
                out.extend(a[code.a..code.ae].iter().map(|line| format!(" {line}")))
            } else {
                if code.tag == "replace" || code.tag == "delete" {
                    out.extend(a[code.a..code.ae].iter().map(|line| format!("-{line}")))
                }
                if code.tag == "replace" || code.tag == "insert" {
                    out.extend(b[code.b..code.be].iter().map(|line| format!("+{line}")))
                }
            }
        }
    }
    Ok(out)
}

fn edit_change(path: &str, old: &Value, new: &Value) -> Result<Value, String> {
    let old = if truthy(old) {
        scalar(old)
    } else {
        String::new()
    };
    let new = if truthy(new) {
        scalar(new)
    } else {
        String::new()
    };
    bounded(&[&old, &new])?;
    let rows = unified(path, &lines(&old), &lines(&new))?;
    Ok(json!({
        "path": path, "operation": "edit", "patch": rows.join("\n"),
        "added": rows.iter().filter(|line| line.starts_with('+') && !line.starts_with("+++")).count(),
        "removed": rows.iter().filter(|line| line.starts_with('-') && !line.starts_with("---")).count(),
        "before_available": true, "after_available": true, "before_complete": false, "after_complete": false,
    }))
}

#[cfg(test)]
mod tests;
