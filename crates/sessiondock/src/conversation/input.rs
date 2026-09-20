//! Positive recognition of the CLI input surface, shared by CHECK and every
//! SEND checkpoint. A nonempty screen alone never authorizes terminal writes.
use crate::delivery::{
    driver::{self, ScreenCapture},
    executor::Failure,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputState {
    Ready,
    Starting,
    Blocked,
    Unknown,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct InputStatus {
    pub state: InputState,
    pub code: &'static str,
    pub message: &'static str,
}
impl InputStatus {
    fn new(state: InputState, code: &'static str, message: &'static str) -> Self {
        Self {
            state,
            code,
            message,
        }
    }
    pub fn ready(self) -> bool {
        self.state == InputState::Ready
    }
    pub fn result(self) -> Result<(), Failure> {
        if self.ready() {
            Ok(())
        } else {
            Err(Failure::new(409, self.code, self.message))
        }
    }
}

/// Screen evidence alone determines AI input readiness. Native history and
/// hook questions are display data and cannot veto a currently writable PTY.
/// Menus precede editor recognition: an overlay may leave an editor visible.
pub(super) fn classify(source: &str, capture: &ScreenCapture) -> InputStatus {
    use InputState::*;
    let ready = InputStatus::new(Ready, "", "");
    if source == "shell" {
        return ready;
    }
    if capture.lag.is_some_and(|lag| lag > 0) {
        return InputStatus::new(Starting, "cli_catching_up", "终端画面正在同步，输入已保留");
    }
    if driver::strip_ansi(&capture.text).trim().is_empty() {
        return InputStatus::new(Starting, "cli_starting", "CLI 正在启动，输入已保留");
    }
    if super::screen_question(capture) {
        return InputStatus::new(
            Blocked,
            "cli_question",
            "CLI 正在等待选择，请切换到 PTY（终端）模式回答；输入已保留",
        );
    }
    if source == "codex" && codex_loading(capture) {
        return InputStatus::new(Starting, "cli_starting", "CLI 正在启动，输入已保留");
    }
    let (recognized, pasting) = match source {
        "claude" | "codex" => {
            let editor = if source == "claude" {
                driver::inspect(capture)
            } else {
                driver::inspect_codex(capture)
            };
            (editor.composer_token.is_some(), editor.pasting)
        }
        "grok" => (grok_composer(capture), false),
        _ => (false, false),
    };
    if !recognized {
        InputStatus::new(
            Unknown,
            "cli_not_ready",
            "未识别到 CLI 可输入的消息编辑区，请切换到 PTY（终端）模式完成登录或处理当前界面后再发送；输入已保留",
        )
    } else if pasting && pasting_indicator(capture) {
        InputStatus::new(Starting, "cli_pasting", "CLI 正在处理粘贴，输入已保留")
    } else {
        ready
    }
}

// Codex paints an editable composer before initialization finishes. In this
// window bracketed paste is accepted but Enter can be ignored. Recognize the
// startup header, not a quoted "loading" in a user's message or transcript.
fn codex_loading(capture: &ScreenCapture) -> bool {
    let text = driver::strip_ansi(&capture.text);
    let mut header = false;
    let mut top_border = false;
    for line in text.lines().take(usize::from(capture.cursor.1)) {
        let row = line.trim();
        if matches!(row.chars().next(), Some('›' | '»')) || (header && row.starts_with('╰')) {
            return false;
        }
        if top_border && row.starts_with("│ >_ OpenAI Codex (") {
            header = true;
        } else if header
            && row
                .strip_prefix('│')
                .and_then(|s| s.trim().strip_prefix("model:"))
                .is_some_and(|s| s.split_whitespace().next() == Some("loading"))
        {
            return true;
        }
        top_border = row.starts_with("╭─");
    }
    false
}

// The old inspector also flags quoted "Pasting…" in transcript/draft text.
// Only the standalone indicator below the cursor is a transient input state.
fn pasting_indicator(capture: &ScreenCapture) -> bool {
    driver::strip_ansi(&capture.text)
        .lines()
        .skip(usize::from(capture.cursor.1) + 1)
        .any(|line| matches!(line.trim(), "Pasting…" | "Pasting..."))
}

pub fn transient_input_error(code: &str) -> bool {
    matches!(code, "cli_starting" | "cli_catching_up" | "cli_pasting")
}

/// Wait only for transient screen evidence, never for menus or unknown UI.
/// No terminal writes occur here, and this never retries a paste or Enter.
pub(super) async fn wait_for_composer<F, Fut>(source: &str, mut capture: F) -> Result<(), Failure>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<ScreenCapture, Failure>>,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let status = classify(source, &capture().await?);
        if status.state != InputState::Starting || tokio::time::Instant::now() >= deadline {
            return status.result();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// A PTY write ack does not mean the CLI has consumed a bracketed paste.
/// Wait for the visible Claude/Codex draft to change, show the end of this
/// prompt, and remain unchanged for 200 ms before allowing Enter.
pub(super) async fn wait_for_pasted_editor<F, Fut>(
    source: &str,
    prompt: &str,
    before: &ScreenCapture,
    mut capture: F,
) -> Result<(), Failure>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<ScreenCapture, Failure>>,
{
    let editor_text = |screen: &ScreenCapture| {
        let view = if source == "codex" {
            driver::inspect_codex(screen)
        } else {
            driver::inspect(screen)
        };
        view.composer_token.and(view.text)
    };
    let original = editor_text(before);
    let suffix: String = prompt
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .rev()
        .take(48)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut stable: Option<(String, tokio::time::Instant)> = None;
    loop {
        let screen = capture().await?;
        let status = classify(source, &screen);
        if status.state == InputState::Blocked {
            return status.result();
        }
        let text =
            if status.ready() && screen.dropped == before.dropped && screen.resets == before.resets
            {
                editor_text(&screen).filter(|text| {
                    Some(text) != original.as_ref()
                        && !suffix.is_empty()
                        && (driver::paste_placeholder(text)
                            || text
                                .chars()
                                .filter(|ch| !ch.is_whitespace())
                                .collect::<String>()
                                .ends_with(&suffix))
                })
            } else {
                None
            };
        match text {
            Some(text) => match &stable {
                Some((previous, since)) if previous == &text => {
                    if since.elapsed() >= Duration::from_millis(200) {
                        return Ok(());
                    }
                }
                _ => stable = Some((text, tokio::time::Instant::now())),
            },
            None => stable = None,
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Failure::new(
                409,
                "cli_pasting",
                "未确认粘贴后的编辑区稳定；未发送回车，输入已保留",
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Grok Build's prompt is a rounded box with a `│ ❯` first row and a
/// model/status label in its lower border. The cursor must be inside that
/// editor, including multiline drafts; a modal or old scrollback box is not
/// an input surface. Layout verified against Grok Build 1.0.34.
fn grok_composer(capture: &ScreenCapture) -> bool {
    let text = driver::strip_ansi(&capture.text);
    let lines: Vec<_> = text.lines().collect();
    let (x, y) = (usize::from(capture.cursor.0), usize::from(capture.cursor.1));
    if y >= lines.len() {
        return false;
    }
    let Some(top) = (0..y).rev().find(|&i| {
        let row = lines[i].trim();
        row.starts_with("╭──")
            && row.ends_with('╮')
            && row[3..row.len() - 3].chars().all(|c| c == '─')
    }) else {
        return false;
    };
    let Some(bottom) = (y + 1..lines.len()).find(|&i| {
        let row = lines[i].trim();
        row.starts_with("╰──") && row.ends_with("─╯")
    }) else {
        return false;
    };
    let left = lines[top].chars().take_while(|c| c.is_whitespace()).count();
    let right = left + lines[top].trim().chars().count() - 1;
    let first = lines[top + 1].trim_start();
    first.starts_with("│ ❯ ")
        && x >= left + if y == top + 1 { 4 } else { 2 }
        && x < right
        && lines[bottom]
            .trim()
            .strip_prefix('╰')
            .and_then(|line| line.strip_suffix('╯'))
            .is_some_and(|line| !line.trim_matches('─').trim().is_empty())
        && (top + 1..bottom)
            .all(|i| lines[i].chars().nth(left) == Some('│') && lines[i].trim_end().ends_with('│'))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(text: &str, cursor: (u16, u16)) -> ScreenCapture {
        ScreenCapture {
            text: text.into(),
            cursor,
            lag: Some(0),
            dropped: Some(0),
            resets: Some(0),
            alt: true,
        }
    }
    fn ensure_composer(source: &str, capture: &ScreenCapture) -> Result<(), Failure> {
        classify(source, capture).result()
    }
    const GROK: &str = "  ╭────────────────────────────────────────────╮\n  │ ❯                                          │\n  ╰────────── Grok 4.6 (low) · always-approve ───╯\n\nLogged in with API key";

    #[test]
    fn pasting_words_in_transcript_or_draft_are_not_a_status() {
        for text in [
            "Pasting…\n────────────────────\n❯ draft\n────────────────────",
            "────────────────────\n❯ Pasting…\n────────────────────",
        ] {
            let y = if text.starts_with("Pasting") { 2 } else { 1 };
            assert!(classify("claude", &frame(text, (10, y))).ready());
        }
    }

    #[test]
    fn codex_multiline_editor_does_not_require_a_footer() {
        let transient = "older output\n\n› Reply with OK.\n  continued line\n\n\n";
        assert!(classify("codex", &frame(transient, (2, 4))).ready());
        let settled =
            format!("{transient}tab to queue message                    100% context left\n");
        assert!(classify("codex", &frame(&settled, (2, 4))).ready());
    }

    #[tokio::test]
    async fn collapsed_multiline_paste_is_stable_editor_evidence() {
        use std::future::ready;
        let rule = "─".repeat(40);
        let before = frame(&format!("welcome\n{rule}\n❯ \n{rule}\n"), (2, 2));
        let pasted = frame(
            &format!(
                "welcome\n{rule}\n❯\u{a0}[Pasted\x1b[Ctext\x1b[C#1\x1b[C+3\x1b[Clines]\n{rule}\n"
            ),
            (27, 2),
        );
        let started = tokio::time::Instant::now();
        wait_for_pasted_editor("claude", "first\nsecond\nthird\nfourth", &before, || {
            ready(Ok(pasted.clone()))
        })
        .await
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn fixture_matrix_matches_input_states() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/composer_input_frames.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let mut capture = frame(
                case["text"].as_str().unwrap(),
                (
                    case["cursor"][0].as_u64().unwrap() as u16,
                    case["cursor"][1].as_u64().unwrap() as u16,
                ),
            );
            capture.lag = Some(case["lag"].as_u64().unwrap_or(0));
            let status =
                serde_json::to_value(classify(case["source"].as_str().unwrap(), &capture)).unwrap();
            assert_eq!(status["state"], case["expected"], "{}", case["name"]);
            assert_eq!(status["code"], case["code"], "{}", case["name"]);
        }
    }

    #[tokio::test]
    async fn transient_frames_wait_but_unknown_and_driver_errors_stop() {
        use std::{collections::VecDeque, future::ready};
        let editor = frame(
            "────────────────────\n❯ draft\n────────────────────\nPasting…",
            (10, 1),
        );
        assert_eq!(classify("claude", &editor).code, "cli_pasting");
        let mut frames = VecDeque::from([
            frame("", (0, 0)),
            editor,
            frame(
                "────────────────────\n❯ message\n────────────────────",
                (9, 1),
            ),
        ]);
        assert!(
            wait_for_composer("claude", || ready(Ok(frames.pop_front().unwrap())))
                .await
                .is_ok()
        );
        assert!(frames.is_empty());
        let mut frames = VecDeque::from([
            frame("", (0, 0)),
            frame("Sign in", (0, 0)),
            frame(GROK, (6, 1)),
        ]);
        let error = wait_for_composer("grok", || ready(Ok(frames.pop_front().unwrap())))
            .await
            .unwrap_err();
        assert_eq!(error.code, "cli_not_ready");
        assert_eq!(frames.len(), 1);
        let error = wait_for_composer("grok", || {
            ready(Err(Failure::new(409, "terminal_unlinked", "gone")))
        })
        .await
        .unwrap_err();
        assert_eq!(error.code, "terminal_unlinked");
    }

    #[test]
    fn login_and_unknown_screens_never_authorize_send() {
        for text in [
            "Approve in your browser to finish signing in.\nMake sure your browser shows this code.\nWaiting for approval...\nctrl+q  quit",
            "Connecting...",
            "Authentication failed",
            "❯ random output",
        ] {
            for source in ["claude", "codex", "grok", "unknown"] {
                let error = ensure_composer(source, &frame(text, (2, 0))).unwrap_err();
                assert_eq!(error.code, "cli_not_ready");
                assert!(error.message.contains("PTY"));
            }
        }
    }
    #[test]
    fn grok_requires_live_box_and_accepts_edits() {
        let mut capture = frame(GROK, (6, 1));
        assert!(ensure_composer("grok", &capture).is_ok());
        capture.text = GROK.replace("│ ❯ ", "│ ❯ hello");
        capture.cursor.0 = 11;
        assert!(ensure_composer("grok", &capture).is_ok());
        capture.text = GROK.replace(' ', "\x1b[C");
        assert!(ensure_composer("grok", &capture).is_ok());
        capture.text = GROK.replace(
            "\n  ╰",
            "\n  │ continued                                  │\n  ╰",
        );
        capture.cursor = (4, 2);
        assert!(ensure_composer("grok", &capture).is_ok());
        capture.text = GROK.replace(" Grok ", " Custom ");
        capture.cursor = (6, 1);
        assert!(ensure_composer("grok", &capture).is_ok());
        capture.text = GROK.into();
        capture.cursor.1 = 4;
        assert!(ensure_composer("grok", &capture).is_err());
        capture.cursor.1 = 1;
        capture.lag = Some(1);
        assert!(ensure_composer("grok", &capture).is_err());
    }
    #[test]
    fn known_claude_codex_editors_allow_busy_input_but_not_other_providers() {
        for (source, capture) in [
            (
                "claude",
                frame(
                    "────────────────────\n❯ message\n────────────────────\nesc to interrupt",
                    (9, 1),
                ),
            ),
            (
                "codex",
                frame(
                    "Working · esc to interrupt\n\n› message\n\ngpt-5.6-luna low · /synthetic/work",
                    (9, 2),
                ),
            ),
        ] {
            assert!(ensure_composer(source, &capture).is_ok(), "{source}");
            assert!(ensure_composer("grok", &capture).is_err());
        }
        assert!(ensure_composer("shell", &frame("$ ", (2, 0))).is_ok());
    }
}
