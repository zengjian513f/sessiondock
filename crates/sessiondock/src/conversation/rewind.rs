//! A rewind made in Claude's own TUI (double Esc, restore the conversation)
//! only moves the CLI's in-memory leaf: nothing reaches the JSONL until the
//! next input (measured on Claude Code 2.1.284). The CLI puts the chosen
//! input back into its editor and redraws the transcript that precedes it.
//! This tells that screen apart from an Esc-returned send and from a history
//! recall (docs/cli-state.md).
use serde_json::Value;

/// Consecutive letters/digits a long record must show on screen to count as
/// visible; shorter assistant text is never matched (the Python rule).
const WINDOW: usize = 48;
/// How far before the rewound input the transcript is searched for a record.
const LOOKBACK: usize = 64;

#[derive(Debug, PartialEq)]
pub enum EditorEcho {
    /// The editor text is no answered input of this view.
    None,
    /// The editor holds an input the CLI already answered, but the screen
    /// does not prove a rewind (a history recall, or nothing recognizable).
    Answered,
    /// The CLI rewound to before this input (its record node, `turn_id`).
    Rewound(String),
}

/// Classifies the editor text against the view's messages (in order) and the
/// transcript above the editor.
pub fn classify(messages: &[&Value], editor: &str, transcript: &str) -> EditorEcho {
    use crate::delivery::driver::same_text_ignoring_whitespace as same;
    if editor.trim().is_empty() {
        return EditorEcho::None;
    }
    let Some(at) = messages.iter().rposition(|message| {
        message["role"] == "user"
            && message["interrupted"] != true
            && message["turn_id"].as_str().is_some_and(|id| !id.is_empty())
            && message["text"]
                .as_str()
                .is_some_and(|text| same(text, editor))
    }) else {
        return EditorEcho::None;
    };
    let answered = messages[at + 1..].iter().any(|message| {
        matches!(
            message["role"].as_str(),
            Some("assistant" | "thinking" | "tool" | "tool_result")
        )
    });
    if !answered {
        return EditorEcho::None;
    }
    let screen = Screen::new(transcript);
    // The input itself or anything after it still on screen: a recall.
    if messages[at..].iter().any(|message| screen.shows(message)) {
        return EditorEcho::Answered;
    }
    // The redrawn transcript must show what preceded the input.
    if messages[..at]
        .iter()
        .rev()
        .take(LOOKBACK)
        .any(|message| screen.shows(message))
    {
        let target = messages[at]["turn_id"].as_str().unwrap_or_default();
        return EditorEcho::Rewound(target.to_owned());
    }
    EditorEcho::Answered
}

/// Letters and digits only: terminal wrapping, Markdown syntax, list bullets
/// and box drawing never break a match.
fn key(text: &str) -> String {
    text.chars().filter(|ch| ch.is_alphanumeric()).collect()
}

struct Screen {
    haystack: String,
    /// Keys of the submitted prompts Claude draws as `❯ text` rows (with
    /// their indented continuation rows).
    prompts: Vec<String>,
}

impl Screen {
    fn new(transcript: &str) -> Self {
        let mut prompts = Vec::new();
        let mut current: Option<String> = None;
        for line in transcript.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix('❯') {
                prompts.extend(current.take());
                current = Some(key(rest));
            } else if let Some(prompt) = current.as_mut() {
                if line.trim().is_empty() || !line.starts_with(' ') {
                    prompts.extend(current.take());
                } else {
                    prompt.push_str(&key(line));
                }
            }
        }
        prompts.extend(current);
        Self {
            haystack: key(transcript),
            prompts,
        }
    }

    fn shows(&self, message: &Value) -> bool {
        let role = message["role"].as_str().unwrap_or("");
        if !matches!(role, "user" | "assistant") {
            return false;
        }
        let needle: Vec<char> = key(message["text"].as_str().unwrap_or(""))
            .chars()
            .collect();
        if needle.len() >= WINDOW {
            return windows(&needle).any(|window| self.haystack.contains(&window));
        }
        role == "user"
            && !needle.is_empty()
            && self
                .prompts
                .iter()
                .any(|prompt| prompt.chars().eq(needle.iter().copied()))
    }
}

/// Overlapping windows every half width, plus the tail window.
fn windows(needle: &[char]) -> impl Iterator<Item = String> + '_ {
    let last = needle.len() - WINDOW;
    (0..=last)
        .step_by(WINDOW / 2)
        .chain(std::iter::once(last))
        .map(move |at| needle[at..at + WINDOW].iter().collect())
}
