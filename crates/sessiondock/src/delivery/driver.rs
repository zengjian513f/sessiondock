//! Terminal driver for the delivery executor.
//!
//! The driver owns nothing durable. It captures the host's screen model,
//! recognizes Claude's composer (rules, `❯`,
//! dim suggestions, cursor position) and Codex's composer the way
//! `codex_bridge.composer_state` does (status footer or cursor-anchored block,
//! `›`/`»` marker, dim placeholder, braille particle glyphs blanked), pastes
//! text, presses keys, and acquires or verifies the instance lease in the same
//! registry the browser uses. It never reads native history, never decides
//! that a prompt was accepted, and never retries a write whose outcome is
//! unknown.

use std::{net::IpAddr, net::Ipv4Addr, sync::Arc};

use futures_util::future::BoxFuture;
use ptyhost_client::BoundTarget;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

use super::claude::ComposerState;
use crate::terminal::{ExpectedTarget, InputPayload, TerminalError, TerminalService};

/// Page identity under which the executor claims a server-held lease. It is a
/// registry page ID like any browser page, so a page holding the lease sees
/// the ordinary conflict and can force it like any takeover.
pub const SERVER_PAGE: &str = "sessiondock-delivery-executor";

/// Managed instance resolved by the executor from the runtime catalog. Never
/// built from a display name, cwd, time or PID.
#[derive(Clone)]
pub struct DeliveryTarget {
    pub name: String,
    /// The uid the host's binding names (a Codex rollback branch delivers
    /// under its ancestor's), which every guarded capture and write asserts.
    pub uid: String,
    pub instance_id: String,
    pub bound: Arc<BoundTarget>,
}

/// A browser page's own lease for the instance, as sent with the request.
/// Used only when it authorizes; otherwise the executor claims for itself.
/// A console opened on a launched (pending) instance holds a launch lease
/// (`launch_id` present); the executor still resolves the native identity
/// through the runtime catalog and only borrows the page's write authority
/// for that exact process instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLease {
    pub page: String,
    pub token: String,
    pub instance_id: String,
    pub launch_id: Option<String>,
}

/// Lease under which one executor operation performs its host requests.
/// `owned` leases were claimed by the executor and are released afterwards;
/// page leases are borrowed and never released by the executor.
pub struct LeaseHandle {
    pub name: String,
    pub uid: String,
    pub instance_id: String,
    launch_id: Option<String>,
    page: String,
    token: String,
    owned: bool,
}

impl LeaseHandle {
    pub fn owned(&self) -> bool {
        self.owned
    }
    pub fn page(&self) -> &str {
        &self.page
    }
    fn expected(&self) -> ExpectedTarget<'_> {
        match &self.launch_id {
            Some(launch) => ExpectedTarget::Launch {
                launch,
                instance: &self.instance_id,
            },
            None => ExpectedTarget::Native {
                uid: &self.uid,
                instance: &self.instance_id,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    /// The host may have executed the request (timeout/EOF after submission).
    pub ambiguous: bool,
    /// Display-only owner of a conflicting lease.
    pub owner_ip: Option<IpAddr>,
}

impl DriverError {
    pub fn ownership(&self) -> bool {
        self.code == "terminal_ownership"
    }
}

impl From<TerminalError> for DriverError {
    fn from(error: TerminalError) -> Self {
        Self {
            status: error.status,
            code: error.code,
            ambiguous: error.code == "terminal_input_ambiguous",
            message: error.message,
            owner_ip: None,
        }
    }
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// One screen capture. `lag`/`dropped` are the host's health counters; when a
/// host does not expose them they are `None`, which is *unknown*, not healthy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenCapture {
    pub text: String,
    pub cursor: (u16, u16),
    pub lag: Option<u64>,
    pub dropped: Option<u64>,
    pub resets: Option<u64>,
    pub alt: bool,
}

/// What the driver could establish about Claude's composer from one capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerView {
    pub state: ComposerState,
    /// Nonreversible fingerprint of screen + cursor; this is
    /// the draft consent token and the domain's frame token.
    pub screen_token: String,
    /// Fingerprint of the composer rows + cursor only, used by the driver's
    /// own compare-and-act before a write when unrelated rows (spinners,
    /// footers) changed but the editor did not.
    pub composer_token: Option<String>,
    /// Visible editor text (soft-wrapped rows joined), when the composer is
    /// recognized.
    pub text: Option<String>,
    pub busy: bool,
    /// The TUI still shows the transient paste-burst indicator; Enter must not
    /// be sent until it clears or the key is swallowed into the paste.
    pub pasting: bool,
    /// The host reported bytes not yet applied to its screen model; the
    /// capture must not be treated as an idle/known composer.
    pub lagging: bool,
    pub dropped: Option<u64>,
}

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))").expect("ansi regex")
});
/// Cursor-forward (`ESC [ n C`). The host's screen model (vt100) writes a
/// styled row's blank cells as these moves instead of spaces, so Claude Code's
/// ` ❯ No, exit` arrives as `\x1b[C❯\x1b[CNo,\x1b[Cexit`; dropping them with the
/// other escapes loses every word gap and column (BUG-20260917-012213-bfffee).
static CUF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[(\d*)C").expect("cuf regex"));
static SGR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\x1b\[([0-9;:]*)m$").expect("sgr"));
static SGR_ANY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;:]*m").expect("sgr any"));
static COMPOSER_RULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*─{12,}(?:\s+.+?\s+─+)?\s*$").expect("rule"));
static BUSY_STATUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?im)(?:\besc\s+to\s+(?:interrupt|stop|cancel)\b|^\s*[✻✽✢✶✳✣✤]\s+\S[^\n]*…(?:\s+\([^\n)]*\))?\s*$|^\s*[✻✽✢✶✳✣✤]\s+[^\n]*\b\d+\s+shells?\s+still\s+running\b[^\n]*$|^\s*\*\s+\S[^\n]*…\s+\([^\n)]*\btokens?\b[^\n)]*\)\s*$)",
    )
    .expect("busy regex")
});

/// The transient paste-burst indicator both TUIs show while a bracketed paste
/// is still being ingested (Claude Code on Windows ConPTY keeps it up well
/// after the text is in the buffer). An Enter sent while it shows is swallowed
/// into the paste, so the composer is not "ready" until it clears
/// (BUG-20260913-093411-0837da).
static PASTING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Pasting[\u{2026}.]+").expect("pasting regex"));

static CODEX_BUSY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bWorking\b.*\besc to interrupt\b").expect("codex busy"));
static CODEX_CONTEXT_FOOTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bContext\s+\d+%\s+used\b").expect("codex context"));
static CODEX_CONTEXT_LEFT_FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:tab\s+to\s+queue\s+message\s+)?\d+%\s+context\s+left\s*$")
        .expect("codex context left")
});
static CODEX_READY_FOOTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bReady\b").expect("codex ready"));
static CODEX_MODEL_FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:gpt|codex|o\d)[\w.-]*(?:\s+\S+)*\s+·\s+\S.*$").expect("codex model")
});
static CODEX_REWIND_FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*esc again to edit previous message\s*$").expect("codex rewind")
});

/// Which CLI's composer model applies to a capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerKind {
    Claude,
    Codex,
}

/// The visible text of a capture: cursor-forward moves become the blank cells
/// they stand for, every other escape (SGR, OSC, other CSI) is removed.
pub fn strip_ansi(text: &str) -> String {
    let spaced = CUF.replace_all(text, |found: &regex::Captures| " ".repeat(cuf_width(found)));
    ANSI.replace_all(&spaced, "").into_owned()
}

/// Cells skipped by one cursor-forward move; an omitted count means one.
fn cuf_width(found: &regex::Captures) -> usize {
    found
        .get(1)
        .map_or("", |m| m.as_str())
        .parse::<usize>()
        .unwrap_or(1)
        .clamp(1, 4096)
}

/// Codex 0.154 animates braille "particles" (U+2800–U+28FF) through the
/// composer's padding rows and the blank cells of its input row in ordinary
/// RGB colours. They are never text: blank them before
/// locating the block and skip them when deciding whether it holds a draft.
fn is_particle(ch: char) -> bool {
    ('\u{2800}'..='\u{28ff}').contains(&ch)
}

/// ANSI stripped, particle cells blanked in place.
fn codex_plain(line: &str) -> String {
    strip_ansi(line)
        .chars()
        .map(|ch| if is_particle(ch) { ' ' } else { ch })
        .collect()
}

/// Whether the visible Codex screen still reports an active turn.
pub fn codex_busy_screen(screen: &str) -> bool {
    CODEX_BUSY.is_match(&strip_ansi(screen))
}

/// Whether Claude visibly has a turn in progress.
pub fn busy_screen(screen: &str) -> bool {
    BUSY_STATUS.is_match(&strip_ansi(screen))
}

/// Visible characters with their SGR dim state.
fn styled_chars(text: &str) -> Vec<(char, bool)> {
    let mut result = Vec::new();
    let mut dim = false;
    let mut last = 0;
    for found in ANSI.find_iter(text) {
        result.extend(text[last..found.start()].chars().map(|ch| (ch, dim)));
        last = found.end();
        if let Some(cuf) = CUF.captures(found.as_str()) {
            // Blank cells keep the current dim state like the spaces they are.
            result.extend(std::iter::repeat_n((' ', dim), cuf_width(&cuf)));
            continue;
        }
        let Some(captures) = SGR.captures(found.as_str()) else {
            continue;
        };
        let body = captures.get(1).map_or("", |m| m.as_str());
        let fields: Vec<&str> = body.split(';').collect();
        let mut at = 0;
        while at < fields.len() {
            let field = fields[at];
            let head = field.split(':').next().unwrap_or("");
            let code: i64 = if head.is_empty() {
                0
            } else {
                match head.parse() {
                    Ok(code) => code,
                    Err(_) => {
                        at += 1;
                        continue;
                    }
                }
            };
            match code {
                0 | 22 => dim = false,
                2 => dim = true,
                _ => {}
            }
            // Numbers inside RGB/256-colour parameters are not SGR dim.
            if matches!(code, 38 | 48 | 58) && !field.contains(':') && at + 1 < fields.len() {
                let mode: i64 = fields[at + 1].parse().unwrap_or(0);
                at += match mode {
                    5 => 2,
                    2 => 4,
                    _ => 0,
                };
            }
            at += 1;
        }
    }
    result.extend(text[last..].chars().map(|ch| (ch, dim)));
    result
}

/// Accept Claude's titled upper border even when a narrow pane clips it.
fn composer_upper(line: &str) -> bool {
    let clean = line.trim_end();
    COMPOSER_RULE.is_match(clean) || (clean.ends_with('─') && clean.contains('─'))
}

struct ComposerBlock {
    prompt_y: usize,
    lower: usize,
    marker: usize,
}

fn locate(clean_lines: &[String], cursor: (u16, u16)) -> Option<ComposerBlock> {
    let (cursor_x, cursor_y) = (cursor.0 as usize, cursor.1 as usize);
    if cursor_y >= clean_lines.len() {
        return None;
    }
    let upper = (0..=cursor_y)
        .rev()
        .find(|&row| composer_upper(&clean_lines[row]))?;
    let lower =
        (cursor_y + 1..clean_lines.len()).find(|&row| COMPOSER_RULE.is_match(&clean_lines[row]))?;
    if lower <= upper + 1 {
        return None;
    }
    let prompt_y = upper + 1;
    let prompt = &clean_lines[prompt_y];
    let marker = prompt.find('❯')?;
    if !prompt[..marker].trim().is_empty() || !(prompt_y <= cursor_y && cursor_y < lower) {
        return None;
    }
    let _ = cursor_x;
    Some(ComposerBlock {
        prompt_y,
        lower,
        marker: prompt[..marker].chars().count(),
    })
}

/// `empty`, `editing` or `unknown`, plus the visible
/// editor text and a composer-only fingerprint when the block is recognized.
pub fn inspect(capture: &ScreenCapture) -> ComposerView {
    let screen_token = screen_fingerprint(&capture.text, capture.cursor);
    let lagging = capture.lag.is_some_and(|lag| lag > 0);
    let busy = busy_screen(&capture.text);
    let pasting = PASTING.is_match(&strip_ansi(&capture.text));
    let unknown = |busy| ComposerView {
        state: ComposerState::Unknown,
        screen_token: screen_token.clone(),
        composer_token: None,
        text: None,
        busy,
        pasting,
        lagging,
        dropped: capture.dropped,
    };
    if lagging {
        return unknown(busy);
    }
    let normalized = capture.text.replace('\r', "");
    let raw_lines: Vec<&str> = normalized.lines().collect();
    let clean_lines: Vec<String> = raw_lines.iter().map(|line| strip_ansi(line)).collect();
    let Some(block) = locate(&clean_lines, capture.cursor) else {
        return unknown(busy);
    };
    let block_raw = raw_lines[block.prompt_y..block.lower].join("\n");
    let styled = styled_chars(&block_raw);
    let Some(marker_at) = styled.iter().position(|(ch, _)| *ch == '❯') else {
        return unknown(busy);
    };
    let content: Vec<(char, bool)> = styled[marker_at + 1..]
        .iter()
        .copied()
        .filter(|(ch, _)| !ch.is_whitespace())
        .collect();
    let composer_token = Some(composer_fingerprint(
        &clean_lines[block.prompt_y..block.lower],
        capture.cursor,
    ));
    let text = composer_text(&clean_lines[block.prompt_y..block.lower], block.marker);
    let (cursor_x, cursor_y) = (capture.cursor.0 as usize, capture.cursor.1 as usize);
    let state = if content.is_empty() {
        ComposerState::Empty
    } else {
        let at_start = cursor_y == block.prompt_y && cursor_x <= block.marker + 2;
        let has_sgr = SGR_ANY.is_match(&block_raw);
        if has_sgr && content.iter().any(|(_, dim)| !dim) {
            ComposerState::Editing
        } else if at_start {
            ComposerState::Empty
        } else {
            ComposerState::Editing
        }
    };
    ComposerView {
        state,
        screen_token,
        composer_token,
        text: Some(text),
        busy,
        pasting,
        lagging,
        dropped: capture.dropped,
    }
}

/// Editor text: the prompt row after `❯ ` plus continuation rows, joined
/// without separators (soft wraps) and end-trimmed per row.
fn composer_text(rows: &[String], marker: usize) -> String {
    let mut out = String::new();
    for (index, row) in rows.iter().enumerate() {
        let body: String = if index == 0 {
            let mut chars = row.chars().skip(marker + 1);
            // Claude renders one space (or NBSP) after the marker.
            match chars.next() {
                Some(' ') | Some('\u{a0}') => chars.collect(),
                Some(other) => std::iter::once(other).chain(chars).collect(),
                None => String::new(),
            }
        } else {
            row.clone()
        };
        out.push_str(body.trim_end());
    }
    out
}

/// Dispatch on the CLI whose composer model applies.
pub fn inspect_for(kind: ComposerKind, capture: &ScreenCapture) -> ComposerView {
    match kind {
        ComposerKind::Claude => inspect(capture),
        ComposerKind::Codex => inspect_codex(capture),
    }
}

fn nonblank(line: &str) -> bool {
    !line.trim().is_empty()
}

/// Locate the Codex composer block `[start, end]`:
/// the nonblank block immediately above a recognized status footer, or, when
/// the short pane hides the footer, the block anchored by the cursor.
fn locate_codex(
    raw_lines: &[&str],
    clean_lines: &[String],
    cursor: (u16, u16),
) -> Option<(usize, usize)> {
    let mut status: Vec<(usize, usize)> = Vec::new();
    for (ready, line) in clean_lines.iter().enumerate() {
        if !CODEX_READY_FOOTER.is_match(line) {
            continue;
        }
        let from = ready.saturating_sub(3);
        if let Some(context) = (from..=ready)
            .rev()
            .find(|&index| CODEX_CONTEXT_FOOTER.is_match(&clean_lines[index]))
        {
            status.push((ready, context));
        }
    }
    let mut footer: Option<usize> = None;
    if let Some(&(ready, context)) = status.last() {
        let mut at = ready.min(context);
        // A narrow terminal may wrap the status bar: exclude its whole
        // nonblank block, or model/account text above Ready looks like a draft.
        while at > 0 && nonblank(&clean_lines[at - 1]) {
            at -= 1;
        }
        footer = Some(at);
    } else if let Some(candidate) = clean_lines.iter().rposition(|line| nonblank(line)) {
        if CODEX_MODEL_FOOTER.is_match(&clean_lines[candidate])
            || CODEX_CONTEXT_LEFT_FOOTER.is_match(&clean_lines[candidate])
        {
            footer = Some(candidate);
        } else if CODEX_REWIND_FOOTER.is_match(&clean_lines[candidate]) {
            // Immediately after Esc, Codex swaps its footer for a dim "esc
            // again to edit previous message" hint; require the native dim
            // styling so quoted transcript text cannot promote an old `›` row.
            let styles: Vec<bool> = styled_chars(raw_lines[candidate])
                .into_iter()
                .filter(|(ch, _)| !ch.is_whitespace() && !is_particle(*ch))
                .map(|(_, dim)| dim)
                .collect();
            if !styles.is_empty() && styles.iter().all(|dim| *dim) {
                footer = Some(candidate);
            }
        }
    }
    let (start, end) = if let Some(footer) = footer {
        // Only the nonblank block immediately above the status bar; a deeper
        // search could reach transcript history mid-redraw. A pasted prompt
        // can contain blank lines, though: in that case the cursor must still
        // sit within the editor, and the nearest preceding › anchors it.
        let mut end = footer.checked_sub(1)?;
        while !nonblank(&clean_lines[end]) {
            end = end.checked_sub(1)?;
        }
        let mut start = end;
        while start > 0 && nonblank(&clean_lines[start - 1]) {
            start -= 1;
        }
        if !matches!(
            clean_lines[start].trim_start().chars().next(),
            Some('›' | '»')
        ) {
            let cursor_y = usize::from(cursor.1);
            // A newline-terminated multiline paste leaves the cursor on its
            // final blank row, even when the status footer remains visible.
            // Keep that row anchored to the editor rather than rejecting it
            // after trimming the blank rows above the footer.
            if cursor_y >= footer || cursor_y > end + 1 {
                return None;
            }
            start = (0..=cursor_y).rev().find(|&index| {
                matches!(
                    clean_lines[index].trim_start().chars().next(),
                    Some('›' | '»')
                )
            })?;
            if cursor_y > end {
                let marker_col = clean_lines[start].chars().count()
                    - clean_lines[start].trim_start().chars().count();
                if start == end || usize::from(cursor.0) != marker_col + 2 {
                    return None;
                }
            }
        }
        (start, end)
    } else {
        // Footerless frame: the host cursor identifies the live block; without
        // it transcript text stays untrusted.
        let (cursor_x, cursor_y) = (cursor.0 as usize, cursor.1 as usize);
        if cursor_y >= clean_lines.len() {
            return None;
        }
        let (start, end) = if nonblank(&clean_lines[cursor_y]) {
            let (mut start, mut end) = (cursor_y, cursor_y);
            while start > 0 && nonblank(&clean_lines[start - 1]) {
                start -= 1;
            }
            if !matches!(
                clean_lines[start].trim_start().chars().next(),
                Some('›' | '»')
            ) {
                start = (0..=cursor_y).rev().find(|&index| {
                    matches!(
                        clean_lines[index].trim_start().chars().next(),
                        Some('›' | '»')
                    )
                })?;
            }
            while end + 1 < clean_lines.len() && nonblank(&clean_lines[end + 1]) {
                end += 1;
            }
            (start, end)
        } else {
            let below = cursor_y
                .checked_sub(1)
                .filter(|&row| nonblank(&clean_lines[row]))
                .and_then(|end| {
                    // A real multiline paste parks Codex's cursor on the first
                    // blank row *below* its footerless editor. Require the editor
                    // to be the final nonblank block, with continuation rows; a
                    // lone old transcript prompt cannot grant SEND readiness.
                    if clean_lines[cursor_y..].iter().any(|line| nonblank(line)) {
                        return None;
                    }
                    // A multiline draft may contain blank paragraphs. The
                    // cursor still anchors its end; find the nearest prompt
                    // marker, as in the footer-backed editor above.
                    let start = (0..=end).rev().find(|&index| {
                        matches!(
                            clean_lines[index].trim_start().chars().next(),
                            Some('›' | '»')
                        )
                    })?;
                    if start == 0
                        || start == end
                        || !matches!(
                            clean_lines[start].trim_start().chars().next(),
                            Some('›' | '»')
                        )
                    {
                        return None;
                    }
                    Some((start, end))
                });
            if let Some(range) = below {
                range
            } else {
                // Codex 0.150.1 can also park the cursor one or two blank
                // rows above a footerless composer on the bottom row.
                let end = clean_lines.len() - 1;
                if !nonblank(&clean_lines[end])
                    || !(1..=2).contains(&(end - cursor_y))
                    || (cursor_y..end).any(|index| nonblank(&clean_lines[index]))
                {
                    return None;
                }
                let mut start = end;
                while start > 0 && nonblank(&clean_lines[start - 1]) {
                    start -= 1;
                }
                (start, end)
            }
        };
        let line = &clean_lines[start];
        let marker_col = line.chars().count() - line.trim_start().chars().count();
        if cursor_x <= marker_col {
            return None;
        }
        if cursor_y < start && cursor_x != marker_col + 2 {
            return None;
        }
        if cursor_y > end && (cursor_y != end + 1 || cursor_x != marker_col + 2) {
            return None;
        }
        (start, end)
    };
    let first = clean_lines[start].trim_start().chars().next()?;
    if !matches!(first, '›' | '»') {
        return None;
    }
    Some((start, end))
}

/// Composer state on one capture: `empty`, `editing` or
/// `unknown`, plus the visible (non-dim, particle-free) editor text and a
/// composer-only fingerprint when the block is recognized. A lagging capture
/// is never a known composer.
pub fn inspect_codex(capture: &ScreenCapture) -> ComposerView {
    let screen_token = screen_fingerprint(&capture.text, capture.cursor);
    let lagging = capture.lag.is_some_and(|lag| lag > 0);
    let busy = codex_busy_screen(&capture.text);
    let pasting = PASTING.is_match(&strip_ansi(&capture.text));
    let unknown = || ComposerView {
        state: ComposerState::Unknown,
        screen_token: screen_token.clone(),
        composer_token: None,
        text: None,
        busy,
        pasting,
        lagging,
        dropped: capture.dropped,
    };
    if lagging {
        return unknown();
    }
    let normalized = capture.text.replace('\r', "");
    let raw_lines: Vec<&str> = normalized.lines().collect();
    let clean_lines: Vec<String> = raw_lines.iter().map(|line| codex_plain(line)).collect();
    let Some((start, end)) = locate_codex(&raw_lines, &clean_lines, capture.cursor) else {
        return unknown();
    };
    let block_raw = raw_lines[start..=end].join("\n");
    let styled = styled_chars(&block_raw);
    let Some(marker_at) = styled.iter().position(|(ch, _)| matches!(ch, '›' | '»')) else {
        return unknown();
    };
    let after = &styled[marker_at + 1..];
    let editing = after
        .iter()
        .any(|(ch, dim)| !ch.is_whitespace() && !is_particle(*ch) && !dim);
    // The draft is whatever is bright after the marker; the dim rotating
    // placeholder and particle cells are not text. Rows keep their newline so
    // soft wraps compare whitespace-insensitively like Claude's.
    let text: String = after
        .iter()
        .filter(|(ch, dim)| is_particle(*ch) || !dim || ch.is_whitespace())
        // Particles occupy blank cells. Deleting them changes whitespace on
        // every animation frame, making an unchanged multiline paste appear
        // unstable forever. Preserve their cells just as codex_plain does.
        .map(|(ch, _)| if is_particle(*ch) { ' ' } else { *ch })
        .collect();
    ComposerView {
        state: if editing {
            ComposerState::Editing
        } else {
            ComposerState::Empty
        },
        screen_token,
        composer_token: Some(composer_fingerprint(
            &clean_lines[start..=end],
            capture.cursor,
        )),
        text: Some(text.trim().to_owned()),
        busy,
        pasting,
        lagging,
        dropped: capture.dropped,
    }
}

/// Fingerprint: sha256 of `x\0y\0screen`.
pub fn screen_fingerprint(screen: &str, cursor: (u16, u16)) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}\0{}\0", cursor.0, cursor.1).as_bytes());
    hasher.update(screen.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn composer_fingerprint(rows: &[String], cursor: (u16, u16)) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("composer\0{}\0{}\0", cursor.0, cursor.1).as_bytes());
    for row in rows {
        hasher.update(row.trim_end().as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

/// Whitespace-insensitive equality used to accept a soft-wrapped composer as
/// showing exactly the payload. Internal whitespace differences (wrap points,
/// NBSP rendering) are ignored; every other character must match in order.
pub fn same_text_ignoring_whitespace(observed: &str, payload: &str) -> bool {
    let a = observed.chars().filter(|ch| !ch.is_whitespace());
    let b = payload.chars().filter(|ch| !ch.is_whitespace());
    a.eq(b)
}

/// Claude Code collapses a multi-line paste into `[Pasted text #1 +N lines]`;
/// Codex can show `[Pasted Content N chars]`. Styled captures may encode the
/// inner spaces as cursor moves, so compare the visible token without spaces.
pub fn paste_placeholder(text: &str) -> bool {
    let text: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    text.starts_with("[Pasted") && text.contains(']')
}

/// Host operations needed by the executor. One production implementation
/// exists; tests provide fakes. Every method must be safe to call only under
/// the executor's per-session serialization.
pub trait TerminalDriver: Send + Sync {
    /// Borrow the page's lease when it authorizes this exact instance;
    /// otherwise claim a server-held lease without force. A lease held by any
    /// other page is the documented ownership conflict.
    fn acquire<'a>(
        &'a self,
        target: &'a DeliveryTarget,
        page: Option<&'a PageLease>,
    ) -> BoxFuture<'a, Result<LeaseHandle, DriverError>>;
    fn capture<'a>(
        &'a self,
        lease: &'a LeaseHandle,
    ) -> BoxFuture<'a, Result<ScreenCapture, DriverError>>;
    fn paste<'a>(
        &'a self,
        lease: &'a LeaseHandle,
        text: &'a str,
    ) -> BoxFuture<'a, Result<(), DriverError>>;
    fn keys<'a>(
        &'a self,
        lease: &'a LeaseHandle,
        keys: &'a [&'static str],
    ) -> BoxFuture<'a, Result<(), DriverError>>;
    /// Release an owned lease; borrowed page leases are left untouched.
    fn release<'a>(&'a self, lease: LeaseHandle) -> BoxFuture<'a, ()>;
}

/// Production driver over the explicit-directory terminal service.
pub struct HostTerminalDriver {
    terminal: Arc<TerminalService>,
}

impl HostTerminalDriver {
    pub fn new(terminal: Arc<TerminalService>) -> Self {
        Self { terminal }
    }

    /// The same lease registry and launch guard, before a native history exists.
    pub async fn acquire_launch(
        &self,
        target: Arc<ptyhost_client::LaunchTarget>,
        page: Option<&PageLease>,
    ) -> Result<LeaseHandle, DriverError> {
        if let Some(page) = page.filter(|p| {
            p.instance_id == target.instance_id()
                && p.launch_id.as_deref() == Some(target.launch_id())
        }) {
            let handle = LeaseHandle {
                name: target.name().into(),
                uid: format!("tmux:{}", target.name()),
                instance_id: target.instance_id().into(),
                launch_id: Some(target.launch_id().into()),
                page: page.page.clone(),
                token: page.token.clone(),
                owned: false,
            };
            match self
                .terminal
                .capture_screen(&handle.name, &handle.page, &handle.token, handle.expected())
                .await
            {
                Ok(_) => return Ok(handle),
                Err(error) if error.code == "terminal_ownership" => {}
                Err(error) => return Err(error.into()),
            }
        }
        let name = target.name().to_owned();
        let instance = target.instance_id().to_owned();
        let launch = target.launch_id().to_owned();
        let response = self
            .terminal
            .claim_launch(target, SERVER_PAGE, IpAddr::V4(Ipv4Addr::LOCALHOST), false)
            .await?;
        let token = response.into_server_token().map_err(|_| DriverError {
            status: 409,
            code: "terminal_ownership",
            message: "终端控制权由其他页面持有".into(),
            ambiguous: false,
            owner_ip: None,
        })?;
        Ok(LeaseHandle {
            uid: format!("tmux:{name}"),
            name,
            instance_id: instance,
            launch_id: Some(launch),
            page: SERVER_PAGE.into(),
            token,
            owned: true,
        })
    }

    async fn claim_server_lease(
        &self,
        target: &DeliveryTarget,
    ) -> Result<LeaseHandle, DriverError> {
        let response = self
            .terminal
            .claim_bound(
                &target.bound,
                SERVER_PAGE,
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                false,
            )
            .await
            .map_err(|error| {
                if error.code == "terminal_ownership" {
                    // A launch (pending-console) lease held by some page is
                    // the same documented conflict; the registry never
                    // downgrades or replaces it without force.
                    DriverError {
                        status: 409,
                        code: "terminal_ownership",
                        message: "终端控制权正由其他页面的控制台租约持有；请从持有控制台的页面发送，或先释放/接管该控制台".into(),
                        ambiguous: false,
                        owner_ip: None,
                    }
                } else {
                    error.into()
                }
            })?;
        match response.into_server_token() {
            Ok(token) => Ok(LeaseHandle {
                name: target.name.clone(),
                uid: target.uid.clone(),
                instance_id: target.instance_id.clone(),
                launch_id: None,
                page: SERVER_PAGE.to_owned(),
                token,
                owned: true,
            }),
            Err(owner) => Err(DriverError {
                status: 409,
                code: "terminal_ownership",
                message: format!(
                    "终端控制权正由其他页面持有（{}）；请从持有控制台的页面发送，或先释放/接管该控制台",
                    owner.describe()
                ),
                ambiguous: false,
                owner_ip: Some(owner.ip),
            }),
        }
    }
}

impl TerminalDriver for HostTerminalDriver {
    fn acquire<'a>(
        &'a self,
        target: &'a DeliveryTarget,
        page: Option<&'a PageLease>,
    ) -> BoxFuture<'a, Result<LeaseHandle, DriverError>> {
        Box::pin(async move {
            if let Some(page) = page.filter(|page| page.instance_id == target.instance_id) {
                let handle = LeaseHandle {
                    name: target.name.clone(),
                    uid: target.uid.clone(),
                    instance_id: target.instance_id.clone(),
                    launch_id: page.launch_id.clone(),
                    page: page.page.clone(),
                    token: page.token.clone(),
                    owned: false,
                };
                // A capture both validates the lease and warms nothing: the
                // executor captures again for its own inspection.
                match self
                    .terminal
                    .capture_screen(&handle.name, &handle.page, &handle.token, handle.expected())
                    .await
                {
                    Ok(_) => return Ok(handle),
                    Err(error) if error.code == "terminal_ownership" => {}
                    Err(error) => return Err(error.into()),
                }
            }
            self.claim_server_lease(target).await
        })
    }

    fn capture<'a>(
        &'a self,
        lease: &'a LeaseHandle,
    ) -> BoxFuture<'a, Result<ScreenCapture, DriverError>> {
        Box::pin(async move {
            let reply = self
                .terminal
                .capture_screen(&lease.name, &lease.page, &lease.token, lease.expected())
                .await?;
            Ok(ScreenCapture {
                text: reply.text,
                cursor: (reply.cursor[0], reply.cursor[1]),
                lag: reply.lag,
                dropped: reply.dropped,
                resets: reply.resets,
                alt: reply.alt,
            })
        })
    }

    fn paste<'a>(
        &'a self,
        lease: &'a LeaseHandle,
        text: &'a str,
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async move {
            self.terminal
                .send_input(
                    &lease.name,
                    &lease.page,
                    &lease.token,
                    lease.expected(),
                    InputPayload::Paste(text.to_owned()),
                )
                .await
                .map(|_| ())
                .map_err(DriverError::from)
        })
    }

    fn keys<'a>(
        &'a self,
        lease: &'a LeaseHandle,
        keys: &'a [&'static str],
    ) -> BoxFuture<'a, Result<(), DriverError>> {
        Box::pin(async move {
            self.terminal
                .send_input(
                    &lease.name,
                    &lease.page,
                    &lease.token,
                    lease.expected(),
                    InputPayload::Keys(keys.iter().copied().map(str::to_owned).collect()),
                )
                .await
                .map(|_| ())
                .map_err(DriverError::from)
        })
    }

    fn release<'a>(&'a self, lease: LeaseHandle) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if lease.owned {
                self.terminal
                    .cancel_reservation(&lease.name, &lease.page, &lease.token);
            }
        })
    }
}

#[cfg(all(test, unix))]
pub(crate) fn test_lease(name: &str, uid: &str, instance: &str) -> LeaseHandle {
    LeaseHandle {
        name: name.into(),
        uid: uid.into(),
        instance_id: instance.into(),
        launch_id: None,
        page: "test-page".into(),
        token: "0".repeat(64),
        owned: false,
    }
}

#[cfg(test)]
mod tests;
