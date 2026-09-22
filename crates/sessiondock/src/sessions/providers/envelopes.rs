//! Source-specific display envelopes. Never interpret arbitrary HTML as protocol
//! or grant file access from a path mentioned inside a textual envelope.
use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

pub(super) fn whole<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.trim()
        .strip_prefix(&format!("<{name}>"))?
        .strip_suffix(&format!("</{name}>"))
}

pub(in crate::sessions) fn recommended_plugins(text: &str) -> bool {
    whole(text, "recommended_plugins").is_some_and(|body| {
        body.trim_start()
            .starts_with("Here is a list of plugins that are available but not installed.")
    })
}

pub(super) fn fork_boilerplate(text: &str) -> bool {
    whole(text, "fork-boilerplate")
        .is_some_and(|body| body.trim_start().starts_with("You are a worker fork."))
}

/// Only text delimiters enclosing actual native image blocks qualify. A text
/// path or an example containing `<image>` never becomes a native image.
pub(in crate::sessions) fn codex_image_wrappers(content: &Value) -> HashSet<usize> {
    static OPEN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^<image name=\[Image #[0-9]+\] path="[^"]+">$"#).unwrap());
    let mut wrappers = HashSet::new();
    let Some(parts) = content.as_array() else {
        return wrappers;
    };
    for (i, part) in parts.iter().enumerate() {
        if !text_part(part).is_some_and(|text| OPEN.is_match(text.trim())) {
            continue;
        }
        let mut end = i + 1;
        while parts
            .get(end)
            .is_some_and(super::image_content::image_shape)
        {
            end += 1;
        }
        if end > i + 1
            && parts
                .get(end)
                .and_then(text_part)
                .is_some_and(|text| text.trim() == "</image>")
        {
            wrappers.insert(i);
            wrappers.insert(end);
        }
    }
    wrappers
}

fn text_part(part: &Value) -> Option<&str> {
    matches!(part["type"].as_str(), Some("text" | "input_text"))
        .then(|| part["text"].as_str())
        .flatten()
}

pub(in crate::sessions) fn codex_title_content(content: &Value) -> Value {
    let wrappers = codex_image_wrappers(content);
    match content.as_array() {
        Some(parts) if !wrappers.is_empty() => Value::Array(
            parts
                .iter()
                .enumerate()
                .filter(|(i, _)| !wrappers.contains(i))
                .map(|(_, part)| part.clone())
                .collect(),
        ),
        _ => content.clone(),
    }
}

/// Read a complete sequence of plain tag/value fields. Text outside that
/// sequence, malformed tags and unknown structures remain available verbatim.
pub(super) fn fields(mut text: &str) -> Option<Vec<(&str, &str)>> {
    let mut result = Vec::new();
    while !text.trim().is_empty() {
        text = text.trim_start();
        let end = text.strip_prefix('<')?.find('>')? + 1;
        let name = &text[1..end];
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return None;
        }
        let body = &text[end + 1..];
        let close = format!("</{name}>");
        let stop = body.find(&close)?;
        result.push((name, &body[..stop]));
        text = &body[stop + close.len()..];
    }
    Some(result)
}

fn labeled_fields(text: &str, labels: &[(&str, &str)]) -> Option<String> {
    let parts = fields(text)?;
    let mut out = Vec::new();
    for (name, body) in parts {
        let label = labels.iter().find(|(key, _)| *key == name)?.1;
        out.push(format!("{label}：{}", body.trim()));
    }
    Some(out.join("\n"))
}

pub(super) fn notification_details(text: &str) -> Option<String> {
    let Some(parts) = whole(text, "task-notification").and_then(fields) else {
        // An incomplete/extended record must not silently lose its payload.
        return Some(text.to_owned());
    };
    let mut out = Vec::new();
    for (name, body) in parts {
        let body = html_escape::decode_html_entities(body.trim()).into_owned();
        if body.is_empty() {
            continue;
        }
        let rendered = match name {
            "summary" | "status" => continue,
            "result" | "event" => body,
            "note" => format!("说明：{body}"),
            "task-id" => format!("任务：{body}"),
            "tool-use-id" => format!("工具调用：{body}"),
            "output-file" => format!("输出文件：{body}"),
            "usage" => labeled_fields(
                &body,
                &[
                    ("subagent_tokens", "Token"),
                    ("tool_uses", "工具调用次数"),
                    ("duration_ms", "耗时（毫秒）"),
                    ("agent_count", "代理数"),
                    ("agents_done", "完成代理"),
                    ("agents_error", "失败代理"),
                    ("agents_skipped", "跳过代理"),
                    ("agents_empty_result", "空结果代理"),
                ],
            )
            .unwrap_or_else(|| format!("<usage>{body}</usage>")),
            "worktree" => labeled_fields(
                &body,
                &[("worktreePath", "工作树"), ("worktreeBranch", "分支")],
            )
            .unwrap_or_else(|| format!("<worktree>{body}</worktree>")),
            _ => format!("<{name}>{body}</{name}>"),
        };
        out.push(rendered);
    }
    (!out.is_empty()).then(|| out.join("\n\n"))
}

/// Normalize tool output only after native error/exit metadata was extracted.
/// The body (including truncation notices) remains text; paths grant nothing.
pub(super) fn tool_text(source: &str, name: Option<&str>, text: &str) -> String {
    if source == "claude" {
        let mut rest = text;
        let mut leading = Vec::new();
        // Native Read can put reminders before the numbered file output.
        while let Some(body) = rest.trim_start().strip_prefix("<system-reminder>") {
            let Some(end) = body.find("</system-reminder>") else {
                break;
            };
            let tail = &body[end + "</system-reminder>".len()..];
            if !tail.is_empty() && !tail.starts_with(['\r', '\n']) {
                break;
            }
            leading.push(format!(
                "系统提示：\n{}",
                body[..end].trim_matches(['\r', '\n'])
            ));
            rest = tail;
        }
        let mut trailing = Vec::new();
        // Preserve output between the two boundaries. Never remove reminders
        // mentioned inline or inside a fenced code example.
        while let Some(start) = rest.rfind("<system-reminder>") {
            if start != 0 && !rest[..start].ends_with('\n') {
                break;
            }
            let Some(body) = whole(&rest[start..], "system-reminder") else {
                break;
            };
            trailing.push(format!("系统提示：\n{}", body.trim_matches(['\r', '\n'])));
            rest = &rest[..start];
        }
        let mut body = rest.to_owned();
        for tag in ["tool_use_error", "persisted-output"] {
            if let Some(inner) = whole(rest, tag) {
                body = inner.trim_matches(['\r', '\n']).to_owned();
                break;
            }
        }
        if name == Some("TaskOutput")
            && rest.trim_start().starts_with("<retrieval_status>")
            && let Some(result) = labeled_fields(
                rest,
                &[
                    ("retrieval_status", "获取状态"),
                    ("task_id", "任务"),
                    ("task_type", "任务类型"),
                    ("status", "任务状态"),
                    ("output", "输出"),
                    ("error", "错误"),
                    ("exit_code", "退出码"),
                ],
            )
        {
            body = result;
        }
        if leading.is_empty() && trailing.is_empty() {
            return body;
        }
        if !body.is_empty() {
            leading.push(body.trim_matches(['\r', '\n']).to_owned());
        }
        leading.extend(trailing.into_iter().rev());
        return leading.join("\n");
    } else if source == "grok" {
        static WORKSPACE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r#"(?s)^\s*<workspace_result workspace_path="([^"]*)">(.*)</workspace_result>\s*$"#,
            )
            .unwrap()
        });
        if let Some(capture) = WORKSPACE.captures(text) {
            return format!(
                "工作区：{}\n{}",
                &capture[1],
                capture[2].trim_matches(['\r', '\n'])
            );
        }
        if text.trim_start().starts_with("<task-id>")
            && let Some(parts) = fields(text)
            && parts.iter().any(|(key, _)| *key == "task-type")
            && let Some(result) = labeled_fields(
                text,
                &[
                    ("task-id", "任务"),
                    ("task-type", "任务类型"),
                    ("output-file", "输出文件"),
                    ("status", "任务状态"),
                    ("summary", "摘要"),
                    ("output", "输出"),
                    ("exit-code", "退出码"),
                ],
            )
        {
            return result;
        }
    }
    text.to_owned()
}
