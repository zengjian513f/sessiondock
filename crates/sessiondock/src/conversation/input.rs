//! Positive recognition of the CLI input surface, shared by CHECK and every
//! SEND checkpoint. A nonempty screen alone never authorizes terminal writes.
use crate::delivery::{
    driver::{self, ScreenCapture},
    target::Failure,
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

/// Text already in the Claude/Codex editor (typed in the PTY, or a prompt the
/// CLI put back after Esc) blocks SEND: the paste would be appended to it and
/// submitted as one message (BUG-20260928-235840-93d732). Python likewise
/// refused a nonempty editor (`draft_conflict`).
pub const INPUT_PENDING: &str = "cli_input_pending";
pub const INPUT_RETURNED: &str = "cli_input_returned";

pub(super) fn input_pending() -> InputStatus {
    InputStatus::new(
        InputState::Blocked,
        INPUT_PENDING,
        "终端输入框里已有未发送的文字，请切换到 PTY（终端）发送或清空后再发送；输入已保留",
    )
}

/// The editor holds exactly a message this service just sent: Claude puts a
/// prompt back when Esc interrupts it before any output, although its user
/// record stays in native history.
pub(super) fn input_returned() -> InputStatus {
    InputStatus::new(
        InputState::Blocked,
        INPUT_RETURNED,
        "上一条消息已被 Esc 退回终端输入框，CLI 未处理；请切换到 PTY（终端）按回车重发或清空后再发送；输入已保留",
    )
}

/// Screen evidence alone determines AI input readiness. Native history and
/// hook questions are display data and cannot veto a currently writable PTY.
/// Menus precede editor recognition: an overlay may leave an editor visible.
pub(super) fn classify(source: &str, capture: &ScreenCapture) -> InputStatus {
    classify_with(source, capture, false)
}

/// `draft_allowed` is only for the checks after SEND's own paste, when the
/// editor text is this message.
fn classify_with(source: &str, capture: &ScreenCapture, draft_allowed: bool) -> InputStatus {
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
    if crate::bridge::menus::screen_prompt(source, &capture.text).is_some()
        || super::screen_question(capture)
        || (source == "opencode" && opencode_dialog(capture))
    {
        return InputStatus::new(
            Blocked,
            "cli_question",
            "CLI 正在等待选择，请切换到 PTY（终端）模式回答；输入已保留",
        );
    }
    if source == "codex" && codex_loading(capture) {
        return InputStatus::new(Starting, "cli_starting", "CLI 正在启动，输入已保留");
    }
    let (recognized, pasting, draft) = match source {
        "claude" | "codex" => {
            let editor = if source == "claude" {
                driver::inspect(capture)
            } else {
                driver::inspect_codex(capture)
            };
            (
                editor.composer_token.is_some(),
                editor.pasting,
                editor.state == driver::ComposerState::Editing,
            )
        }
        "grok" => (grok_composer(capture), false, false),
        "opencode" => (opencode_editor(capture), false, false),
        "agy" => {
            let text = agy_editor(capture);
            (
                text.is_some(),
                false,
                text.is_some_and(|text| !text.trim().is_empty()),
            )
        }
        _ => (false, false, false),
    };
    if !recognized {
        InputStatus::new(
            Unknown,
            "cli_not_ready",
            "未识别到 CLI 可输入的消息编辑区，请切换到 PTY（终端）模式完成登录或处理当前界面后再发送；输入已保留",
        )
    } else if pasting && pasting_indicator(capture) {
        InputStatus::new(Starting, "cli_pasting", "CLI 正在处理粘贴，输入已保留")
    } else if draft && !draft_allowed {
        input_pending()
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
/// prompt, and remain unchanged for 200 ms before allowing Enter. Like
/// Python's `wait_paste_consumed`, this wait is best effort: when the CLI
/// repaints slowly the deadline still lets Enter through. Refusing it left the
/// pasted draft in the CLI and made every retry of the same submission fail
/// (BUG-20260927-112827-5aa96e). Only a menu vetoes Enter.
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
        if source == "agy" {
            return agy_editor(screen);
        }
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
        let status = classify_with(source, &screen, true);
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
            return Ok(());
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

/// OpenCode's question form and permission prompt replace the prompt block;
/// each ends with its key-hint footer on a `┃` row near the bottom of the
/// screen (verified against OpenCode 2.0.18). The same words quoted in the
/// transcript sit above the prompt block, not in its last rows.
fn opencode_dialog(capture: &ScreenCapture) -> bool {
    let text = driver::strip_ansi(&capture.text);
    let rows: Vec<&str> = text.lines().filter(|row| !row.trim().is_empty()).collect();
    rows.iter().rev().take(4).any(|row| {
        let row = row.trim();
        row.starts_with('┃')
            && ((row.contains("enter submit") && row.contains("esc dismiss"))
                || (row.contains("Allow once")
                    && row.contains("Reject")
                    && row.contains("enter confirm")))
    })
}

/// OpenCode's prompt is a block of rows with a single `┃` left border, closed
/// by a `╹▀…` rule; its last row names the agent and model (`Build · …`). The
/// cursor must be on a draft row inside that block: the command palette and
/// dialogs move it away, and completion popups are bordered on both sides.
/// Layout verified against OpenCode 1.18.32.
fn opencode_editor(capture: &ScreenCapture) -> bool {
    let text = driver::strip_ansi(&capture.text);
    let lines: Vec<Vec<char>> = text.lines().map(|line| line.chars().collect()).collect();
    let (x, y) = (usize::from(capture.cursor.0), usize::from(capture.cursor.1));
    let Some(left) = lines
        .get(y)
        .and_then(|row| row.iter().position(|&c| c == '┃'))
    else {
        return false;
    };
    let boxed = |i: usize| {
        lines.get(i).is_some_and(|row| {
            row.get(left) == Some(&'┃')
                && row[..left].iter().all(|c| c.is_whitespace())
                && row.iter().rposition(|&c| c == '┃') == Some(left)
        })
    };
    if !boxed(y) || x < left + 3 {
        return false;
    }
    let last = (y..lines.len())
        .take_while(|&i| boxed(i))
        .last()
        .unwrap_or(y);
    let label: String = lines[last][left + 1..].iter().collect();
    y < last
        && lines
            .get(last + 1)
            .is_some_and(|rule| rule.get(left) == Some(&'╹') && rule.get(left + 1) == Some(&'▀'))
        && label
            .trim_start()
            .split_once(" · ")
            .is_some_and(|(agent, model)| !agent.trim().is_empty() && !model.trim().is_empty())
}

/// Agy 1.2.16/1.2.17: a `> ` editor between full horizontal rules, followed
/// by an optional running-task panel and a footer. Menus move the cursor out
/// of the editor; arbitrary output below it is not a task panel.
fn agy_editor_layout(capture: &ScreenCapture) -> Option<(String, String, bool)> {
    let text = driver::strip_ansi(&capture.text);
    let rows: Vec<&str> = text.lines().collect();
    let (x, y) = (usize::from(capture.cursor.0), usize::from(capture.cursor.1));
    if y >= rows.len() || x < 2 {
        return None;
    }
    let rule =
        |row: &str| row.trim().chars().count() >= 8 && row.trim().chars().all(|ch| ch == '─');
    let top = (0..y).rev().find(|&i| rule(rows[i]))?;
    let bottom = (y + 1..rows.len()).find(|&i| rule(rows[i]))?;
    if !rows[top + 1].starts_with('>') || rows[top].trim() != rows[bottom].trim() {
        return None;
    }
    let tail: Vec<&str> = rows[bottom + 1..]
        .iter()
        .copied()
        .filter(|row| !row.trim().is_empty())
        .collect();
    let background = tail.len() > 1;
    let footer = if background {
        // Captured 1.2.17 panel: one or more `  ● [HH:MM:SS] … running`
        // rows, the same full rule, then the normal footer with /tasks.
        let task = |row: &str| {
            let Some(rest) = row.strip_prefix("  ● [") else {
                return false;
            };
            let Some((time, command)) = rest.split_once("] ") else {
                return false;
            };
            let parts: Vec<&str> = time.split(':').collect();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit()))
                && command
                    .strip_suffix(" running")
                    .is_some_and(|s| !s.is_empty())
        };
        if tail.len() < 3
            || tail[tail.len() - 2].trim() != rows[bottom].trim()
            || !tail[..tail.len() - 2].iter().all(|row| task(row))
        {
            return None;
        }
        let footer = tail[tail.len() - 1];
        if !footer.trim_end().ends_with(" · /tasks") {
            return None;
        }
        footer
    } else {
        tail.first().copied().unwrap_or("")
    };
    let mut lines = Vec::new();
    for (i, row) in rows[top + 1..bottom].iter().enumerate() {
        let content = if i == 0 {
            if *row == ">" {
                ""
            } else {
                row.strip_prefix("> ")?
            }
        } else {
            row.strip_prefix("  ")
                .or_else(|| row.trim().is_empty().then_some(""))?
        };
        lines.push(content.trim_end());
    }
    Some((
        lines.join("\n").trim_end().to_owned(),
        footer.to_owned(),
        background,
    ))
}

pub(super) fn agy_editor(capture: &ScreenCapture) -> Option<String> {
    agy_editor_layout(capture).map(|(editor, _, _)| editor)
}

/// Agy 1.2.16's footer belongs below the verified editor, not to the
/// transcript or draft. A model label is right-aligned on the same row.
pub(super) fn agy_busy(capture: &ScreenCapture) -> Option<bool> {
    let (editor, footer, background) = agy_editor_layout(capture)?;
    let label = |expected: &str| {
        footer == expected
            || footer
                .strip_prefix(expected)
                .is_some_and(|tail| tail.starts_with(' '))
    };
    if background || label("esc to cancel") {
        Some(true)
    } else if label("? for shortcuts") || (!editor.is_empty() && footer.starts_with(' ')) {
        Some(false)
    } else {
        None
    }
}
