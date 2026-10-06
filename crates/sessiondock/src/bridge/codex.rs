//! Codex command approvals.
//!
//! Codex writes `request_user_input` questions to its rollout, but a command
//! approval lives only on the TUI screen. This parser is deliberately strict:
//! the exact footer, at least two numbered options with `(y)`/`(p)`/`(esc)`
//! shortcuts, both a positive and the negative path, and nothing printed after
//! the footer — ordinary output that quotes the heading is never a prompt.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::delivery::driver::strip_ansi;

/// Folder trust precedes the first rollout. Recognition never grants trust;
/// keep the disclosure and paths exactly as shown on the live screen.
pub fn startup_prompt(screen: &str) -> Option<Value> {
    let clean = strip_ansi(screen).replace('\r', "");
    let lines: Vec<_> = clean.lines().map(str::trim_end).collect();
    let start = lines
        .iter()
        .rposition(|line| line.trim() == "Folder access")?;
    let block = &lines[start..];
    let footer = block
        .iter()
        .position(|line| line.trim() == "enter continue · esc quit")?;
    if block[footer + 1..]
        .iter()
        .any(|line| !line.trim().is_empty())
    {
        return None;
    }
    let options: Vec<_> = block[..footer]
        .iter()
        .filter_map(|line| {
            OPTION
                .captures(line)
                .map(|captures| (captures[1].to_owned(), captures[2].trim().to_owned()))
        })
        .collect();
    if options
        != [
            ("1".into(), "Trust and continue".into()),
            ("2".into(), "Quit".into()),
        ]
        || !block
            .iter()
            .any(|line| line.trim_start().starts_with("Trust this folder?"))
        || !block.iter().any(|line| line.trim_start().starts_with('›'))
    {
        return None;
    }
    let end = block.iter().position(|line| OPTION.is_match(line))?;
    let question = block[1..end]
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    let digest = format!("{:x}", Sha256::digest(question.as_bytes()));
    Some(json!({
        "id": format!("codex-startup:{}", &digest[..16]),
        "source": "codex", "kind": "folder_trust", "state": "waiting",
        "questions": [{"header": "目录信任确认", "question": question, "multiple": false,
            "options": [{"label": "信任并继续", "keys": ["1", "Enter"]},
                        {"label": "退出", "keys": ["2"]}]}],
    }))
}

static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(Would you like to .+\?)\s*$").expect("heading"));
static OPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[›»>]\s*)?(\d+)\.\s+(.*)$").expect("option"));
static FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*Press enter to confirm or esc to cancel\s*$").expect("footer")
});
static SHORTCUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*\((y|p|esc)\)\s*$").expect("shortcut"));

/// Compact Chinese labels that keep Codex's meaning.
fn option_text(text: &str, key: &str) -> (String, String) {
    let folded = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let low = folded.to_lowercase();
    if key == "y" || low.starts_with("yes, proceed") {
        return ("允许本次".into(), "运行这条命令".into());
    }
    if key == "p" || low.contains("don't ask again") {
        return ("始终允许".into(), "以后不再询问这个命令前缀".into());
    }
    if key == "esc" || low.starts_with("no,") {
        return ("拒绝".into(), "取消命令，并告诉 Codex 调整方案".into());
    }
    (folded, String::new())
}

/// The live approval at the screen tail as a question-card payload, or
/// `None` when the screen shows no complete, current approval dialog.
pub fn approval_prompt(screen: &str) -> Option<Value> {
    let clean = strip_ansi(screen).replace('\r', "");
    let lines: Vec<&str> = clean.split('\n').collect();
    let start = lines.iter().rposition(|line| HEADING.is_match(line))?;
    let block = &lines[start..];
    let footer = block.iter().position(|line| FOOTER.is_match(line))?;
    if block[footer + 1..]
        .iter()
        .any(|line| !line.trim().is_empty())
    {
        return None;
    }
    let block = &block[..=footer];

    let option_starts: Vec<(usize, regex::Captures<'_>)> = block
        .iter()
        .enumerate()
        .filter_map(|(index, line)| OPTION.captures(line).map(|captures| (index, captures)))
        .collect();
    if option_starts.len() < 2 {
        return None;
    }
    let mut options: Vec<Value> = Vec::new();
    for (position, (line_index, captures)) in option_starts.iter().enumerate() {
        let end = option_starts
            .get(position + 1)
            .map_or(footer, |(next, _)| *next);
        let mut parts = vec![captures.get(2).map_or("", |m| m.as_str()).trim().to_owned()];
        parts.extend(
            block[line_index + 1..end]
                .iter()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        );
        let raw = parts.join(" ").trim().to_owned();
        let shortcut = SHORTCUT.captures(&raw)?;
        let key = shortcut.get(1).map_or("", |m| m.as_str()).to_lowercase();
        let raw = SHORTCUT.replace(&raw, "").trim().to_owned();
        let (label, description) = option_text(&raw, &key);
        let mut option = json!({
            "label": label,
            "key": if key == "esc" { "Escape".to_owned() } else { key },
        });
        if !description.is_empty() {
            option["description"] = json!(description);
        }
        options.push(option);
    }
    // Current Codex approvals always expose a positive and a negative path.
    let keys: Vec<&str> = options
        .iter()
        .filter_map(|option| option["key"].as_str())
        .collect();
    if !keys.contains(&"Escape") || !(keys.contains(&"y") || keys.contains(&"p")) {
        return None;
    }

    let heading = HEADING
        .captures(block[0])?
        .get(1)
        .map_or("", |m| m.as_str())
        .to_owned();
    let mut environment = String::new();
    let mut command_lines: Vec<String> = Vec::new();
    let mut in_command = false;
    for line in &block[1..option_starts[0].0] {
        let stripped = line.trim();
        if stripped.to_lowercase().starts_with("environment:") {
            environment = stripped
                .split_once(':')
                .map_or("", |(_, value)| value)
                .trim()
                .to_owned();
        }
        if stripped.starts_with('$') {
            in_command = true;
            command_lines.push(stripped.to_owned());
        } else if in_command && !stripped.is_empty() {
            command_lines.push(stripped.to_owned());
        }
    }
    let command = command_lines.join(" ");
    let mut question = if heading.to_lowercase().contains("run the following command") {
        "是否运行以下命令？".to_owned()
    } else {
        heading.clone()
    };
    if !command.is_empty() {
        question.push_str("\n\n");
        question.push_str(&command);
    }
    let mut digest_source = vec![heading.clone(), environment.clone(), command];
    digest_source.extend(options.iter().map(|option| {
        format!(
            "{}:{}",
            option["key"].as_str().unwrap_or(""),
            option["label"].as_str().unwrap_or("")
        )
    }));
    let digest = format!("{:x}", Sha256::digest(digest_source.join("\n").as_bytes()));
    Some(json!({
        "id": format!("codex-approval:{}", &digest[..16]),
        "state": "waiting",
        "kind": "approval",
        "questions": [{
            "header": if environment.is_empty() {
                "命令审批".to_owned()
            } else {
                format!("命令审批 · {environment}")
            },
            "question": question,
            "options": options,
            "multiple": false,
        }],
    }))
}
