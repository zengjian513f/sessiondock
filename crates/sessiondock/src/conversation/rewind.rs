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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const EARLIER: &str = "Two things will make noise. One can be designed to be quiet, the other needs a real measurement first.";
    const LATEST: &str = "I cannot connect this message to anything here: the whole session discussed context usage and compaction.";

    fn view() -> Vec<Value> {
        vec![
            json!({"role": "user", "turn_id": "u1", "text": "will the hook be noisy"}),
            json!({"role": "assistant", "turn_id": "u1", "text": EARLIER}),
            json!({"role": "user", "turn_id": "u2", "text": "ok, support drag selection"}),
            json!({"role": "thinking", "turn_id": "u2", "text": ""}),
            json!({"role": "assistant", "turn_id": "u2", "text": LATEST}),
        ]
    }

    fn classify_view(editor: &str, transcript: &str) -> EditorEcho {
        let view = view();
        let messages: Vec<&Value> = view.iter().collect();
        classify(&messages, editor, transcript)
    }

    #[test]
    fn redrawn_transcript_before_the_editor_input_is_a_rewind() {
        // Claude wraps and renders Markdown; only letters and digits count.
        let transcript = "❯ will the hook be noisy\n\n● Two things will make\n  noise. One can be **designed** to be quiet, the other needs a real\n  measurement first.\n\n✻ Cogitated for 46s";
        assert_eq!(
            classify_view("ok, support drag  selection", transcript),
            EditorEcho::Rewound("u2".into())
        );
    }

    #[test]
    fn a_recall_below_the_answer_is_not_a_rewind() {
        let transcript = format!(
            "❯ will the hook be noisy\n\n● {EARLIER}\n\n❯ ok, support drag selection\n\n● {LATEST}"
        );
        assert_eq!(
            classify_view("ok, support drag selection", &transcript),
            EditorEcho::Answered
        );
        // The short prompt alone on screen proves it too.
        assert_eq!(
            classify_view(
                "ok, support drag selection",
                "❯ ok, support drag\n  selection\n"
            ),
            EditorEcho::Answered
        );
        // Nothing recognizable: no rewind is claimed.
        assert_eq!(
            classify_view("ok, support drag selection", "FAKE"),
            EditorEcho::Answered
        );
    }

    #[test]
    fn unanswered_or_unknown_editor_text_is_left_alone() {
        let transcript = format!("● {EARLIER}");
        assert_eq!(
            classify_view("something else", &transcript),
            EditorEcho::None
        );
        assert_eq!(classify_view("  ", &transcript), EditorEcho::None);
        let mut view = view();
        view.truncate(3);
        let messages: Vec<&Value> = view.iter().collect();
        assert_eq!(
            classify(&messages, "ok, support drag selection", &transcript),
            EditorEcho::None
        );
    }
}
