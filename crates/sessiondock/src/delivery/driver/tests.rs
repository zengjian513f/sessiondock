use super::*;

const RULE: &str = "────────────────────────────────────────────────";

fn capture(text: &str, cursor: (u16, u16)) -> ScreenCapture {
    ScreenCapture {
        text: text.to_owned(),
        cursor,
        lag: Some(0),
        dropped: Some(0),
        resets: Some(0),
        alt: false,
    }
}

fn fake_screen(buffer: &str, footer: &str) -> String {
    let mut lines = vec![
        "FAKE_CLAUDE_READY sid=[abc]".to_owned(),
        "> earlier prompt".to_owned(),
        String::new(),
        RULE.to_owned(),
        format!("❯ {buffer}"),
        RULE.to_owned(),
    ];
    if !footer.is_empty() {
        lines.push(footer.to_owned());
    }
    lines.join("\n")
}

#[test]
fn empty_composer_is_recognized_with_cursor_after_marker() {
    let view = inspect(&capture(&fake_screen("", ""), (2, 4)));
    assert_eq!(view.state, ComposerState::Empty);
    assert_eq!(view.text.as_deref(), Some(""));
    assert!(view.composer_token.is_some());
    assert!(!view.busy && !view.lagging);
    assert_eq!(
        view.screen_token,
        screen_fingerprint(&fake_screen("", ""), (2, 4))
    );
}

#[test]
fn typed_text_is_editing_and_extracted_exactly() {
    let view = inspect(&capture(&fake_screen("hello  world", ""), (14, 4)));
    assert_eq!(view.state, ComposerState::Editing);
    assert_eq!(view.text.as_deref(), Some("hello  world"));
}

#[test]
fn soft_wrapped_rows_join_without_separator() {
    let screen = [
        RULE,
        "❯ first part of a long",
        "prompt continues here",
        RULE,
    ]
    .join("\n");
    let view = inspect(&capture(&screen, (5, 2)));
    assert_eq!(view.state, ComposerState::Editing);
    assert_eq!(
        view.text.as_deref(),
        Some("first part of a longprompt continues here")
    );
    assert!(same_text_ignoring_whitespace(
        view.text.as_deref().unwrap(),
        "first part of a long prompt continues here"
    ));
    assert!(!same_text_ignoring_whitespace(
        "first part",
        "first part of a long"
    ));
}

#[test]
fn dim_suggestion_with_cursor_at_start_is_empty() {
    // Claude draws its placeholder suggestion dim; a real draft is bright.
    let screen = [RULE, "❯ \x1b[2mTry \"fix the tests\"\x1b[22m", RULE].join("\n");
    assert_eq!(
        inspect(&capture(&screen, (2, 1))).state,
        ComposerState::Empty
    );
    let bright = [RULE, "❯ \x1b[2mTry\x1b[22m real draft", RULE].join("\n");
    assert_eq!(
        inspect(&capture(&bright, (2, 1))).state,
        ComposerState::Editing
    );
    // Without colour information the cursor position decides.
    let plain = [RULE, "❯ static suggestion", RULE].join("\n");
    assert_eq!(
        inspect(&capture(&plain, (2, 1))).state,
        ComposerState::Empty
    );
    assert_eq!(
        inspect(&capture(&plain, (10, 1))).state,
        ComposerState::Editing
    );
}

#[test]
fn colour_parameters_do_not_toggle_dim() {
    let styled = styled_chars("\x1b[38;5;2mA\x1b[0m\x1b[38;2;1;2;3mB\x1b[2mC");
    assert_eq!(styled, vec![('A', false), ('B', false), ('C', true)]);
}

#[test]
fn menus_and_missing_rules_are_unknown() {
    let menu = ["Choose an option", "❯ 1. yes", "  2. no"].join("\n");
    assert_eq!(
        inspect(&capture(&menu, (2, 1))).state,
        ComposerState::Unknown
    );
    let cursor_outside = fake_screen("draft", "");
    assert_eq!(
        inspect(&capture(&cursor_outside, (0, 0))).state,
        ComposerState::Unknown
    );
    let above = [RULE, "❯ x", RULE].join("\n");
    assert_eq!(
        inspect(&capture(&above, (0, 0))).state,
        ComposerState::Unknown
    );
}

#[test]
fn titled_upper_border_clipped_by_narrow_pane_is_accepted() {
    let screen = ["my long renamed session ─", "❯ typed", RULE].join("\n");
    assert_eq!(
        inspect(&capture(&screen, (7, 1))).state,
        ComposerState::Editing
    );
}

#[test]
fn lagging_capture_is_never_treated_as_known() {
    let mut lagging = capture(&fake_screen("", ""), (2, 4));
    lagging.lag = Some(12);
    let view = inspect(&lagging);
    assert_eq!(view.state, ComposerState::Unknown);
    assert!(view.lagging && view.composer_token.is_none());
    // A host without health counters is unknown health, not lag.
    let mut unknown_health = capture(&fake_screen("", ""), (2, 4));
    unknown_health.lag = None;
    unknown_health.dropped = None;
    let view = inspect(&unknown_health);
    assert_eq!(view.state, ComposerState::Empty);
    assert!(!view.lagging && view.dropped.is_none());
}

#[test]
fn busy_footer_detection_matches_python_patterns() {
    assert!(busy_screen("… esc to interrupt …"));
    assert!(busy_screen("✢ Unfurling… (3s)"));
    assert!(busy_screen("✳ Thinking 2 shells still running"));
    assert!(busy_screen("* Working… (12 tokens)"));
    assert!(!busy_screen("* A markdown bullet… (no counter)"));
    assert!(!busy_screen("plain prompt"));
    let view = inspect(&capture(
        &fake_screen("", "✻ Thinking… (esc to interrupt)"),
        (2, 4),
    ));
    assert!(view.busy);
    assert_eq!(view.state, ComposerState::Empty);
}

#[test]
fn the_transient_pasting_indicator_is_flagged_until_it_clears() {
    // The draft is already fully in the composer while the TUI still shows the
    // paste-burst indicator (Windows ConPTY Claude keeps it up after the text
    // lands): the view must report `pasting` so the executor holds Enter.
    let pasting = inspect(&capture(
        &fake_screen("the whole draft", "Pasting…"),
        (17, 4),
    ));
    assert!(pasting.pasting);
    assert_eq!(pasting.state, ComposerState::Editing);
    assert_eq!(pasting.text.as_deref(), Some("the whole draft"));
    // Same composer once the indicator clears: ready for Enter.
    let ready = inspect(&capture(&fake_screen("the whole draft", ""), (17, 4)));
    assert!(!ready.pasting);
    assert_eq!(ready.state, ComposerState::Editing);
    // Trailing-dots spelling and the codex composer are covered too.
    assert!(inspect(&capture(&fake_screen("x", "Pasting..."), (3, 4))).pasting);
    assert!(!inspect(&capture(&fake_screen("x", "plain footer"), (3, 4))).pasting);
}

#[test]
fn fingerprints_cover_cursor_and_screen_and_composer_rows_separately() {
    let a = inspect(&capture(&fake_screen("x", "footer one"), (3, 4)));
    let b = inspect(&capture(&fake_screen("x", "footer two"), (3, 4)));
    let c = inspect(&capture(&fake_screen("x", "footer one"), (2, 4)));
    assert_ne!(a.screen_token, b.screen_token);
    assert_eq!(a.composer_token, b.composer_token);
    assert_ne!(a.screen_token, c.screen_token);
    assert_ne!(a.composer_token, c.composer_token);
    assert_eq!(screen_fingerprint("s", (1, 2)), {
        let mut hasher = Sha256::new();
        hasher.update(b"1\x002\x00s");
        format!("{:x}", hasher.finalize())
    });
}

#[test]
fn ansi_stripping_handles_osc_and_csi() {
    assert_eq!(strip_ansi("\x1b]0;title\x07a\x1b[31mb\x1b[0m"), "ab");
}

/// vt100 styled rows encode blank cells as cursor-forward moves; the strip
/// must give those cells back as spaces so words and columns survive
/// (the real Claude 2.1.274 trust dialog through ptyhost).
#[test]
fn cursor_forward_moves_strip_to_blank_cells() {
    assert_eq!(
        strip_ansi("\x1b[C\x1b[38;2;177;185;249m❯\x1b[CNo,\x1b[Cexit"),
        " ❯ No, exit"
    );
    assert_eq!(
        strip_ansi("\x1b[3CYes,\x1b[CI\x1b[Ctrust\x1b[Cthis\x1b[Cfolder"),
        "   Yes, I trust this folder"
    );
    assert_eq!(
        strip_ansi("a\x1b[12Cb\x1b[0C"),
        format!("a{}b ", " ".repeat(12))
    );
    // Other cursor CSI (up/down/back) still vanish without leaving cells.
    assert_eq!(strip_ansi("a\x1b[2A\x1b[3D\x1b[Bb"), "ab");
    // Dim tracking sees the same cells.
    let dimmed: String = styled_chars("\x1b[2mx\x1b[2Cy\x1b[0m z")
        .iter()
        .map(|(ch, dim)| if *dim { ch.to_ascii_uppercase() } else { *ch })
        .collect();
    assert_eq!(dimmed, "X  Y z");
}

// ---- Codex composer (port of codex_bridge.composer_state) --------

const CODEX_FOOTER: &str = "gpt-5.6-luna low · /synthetic/codex-area";
const PARTICLES: &str = "\x1b[38;2;90;90;90m⠁⠂⠄ ⠈⠐⠠\x1b[0m";

fn codex_screen(input_row: &str, footer: &str) -> String {
    let mut lines = vec![
        "FAKE_CODEX_TUI sid=[abc]".to_owned(),
        "> earlier prompt".to_owned(),
        String::new(),
        PARTICLES.to_owned(),
        input_row.to_owned(),
        PARTICLES.to_owned(),
        String::new(),
    ];
    if !footer.is_empty() {
        lines.push(footer.to_owned());
    }
    lines.join("\n")
}

#[test]
fn codex_dim_placeholder_with_particles_is_empty() {
    let row = "› \x1b[2mAsk Codex to do anything\x1b[0m   \x1b[38;2;90;90;90m⠁⠂\x1b[0m";
    let view = inspect_codex(&capture(&codex_screen(row, CODEX_FOOTER), (2, 4)));
    assert_eq!(view.state, ComposerState::Empty);
    assert_eq!(view.text.as_deref(), Some(""));
    assert!(view.composer_token.is_some());
    assert!(!view.busy && !view.lagging);
    assert_eq!(
        inspect_for(
            ComposerKind::Codex,
            &capture(&codex_screen(row, CODEX_FOOTER), (2, 4))
        )
        .state,
        ComposerState::Empty
    );
}

#[test]
fn codex_bright_draft_is_editing_with_text_and_particles_ignored() {
    // A coloured (not dim) draft with particle cells around it: editing, and
    // the extracted text carries no particle glyphs.
    let row = "› \x1b[38;2;200;200;200mhello  codex\x1b[0m \x1b[38;2;90;90;90m⠁⠂\x1b[0m";
    let view = inspect_codex(&capture(&codex_screen(row, CODEX_FOOTER), (14, 4)));
    assert_eq!(view.state, ComposerState::Editing);
    assert_eq!(view.text.as_deref(), Some("hello  codex"));
    // A particle-only padding row never becomes a draft block start.
    let screen = [PARTICLES, "› real draft", PARTICLES, "", CODEX_FOOTER].join("\n");
    let view = inspect_codex(&capture(&screen, (13, 1)));
    assert_eq!(view.state, ComposerState::Editing);
    assert_eq!(view.text.as_deref(), Some("real draft"));
}

#[test]
fn codex_wrapped_rows_join_and_compare_whitespace_insensitively() {
    let screen = [
        "› first part of a long",
        "  prompt continues here",
        "",
        CODEX_FOOTER,
    ]
    .join("\n");
    let view = inspect_codex(&capture(&screen, (5, 1)));
    assert_eq!(view.state, ComposerState::Editing);
    assert!(same_text_ignoring_whitespace(
        view.text.as_deref().unwrap(),
        "first part of a long prompt continues here"
    ));
}

#[test]
fn codex_ready_context_footer_and_rewind_hint_locate_the_block() {
    // The composer block sits above a blank row and the (possibly wrapped)
    // status bar; the bar's own nonblank block is excluded.
    let screen = [
        "transcript line",
        "",
        "» draft via ready footer",
        "",
        "gpt-5.6-luna · account   Context 12% used",
        "Ready",
    ]
    .join("\n");
    let view = inspect_codex(&capture(&screen, (25, 2)));
    assert_eq!(view.state, ComposerState::Editing);
    assert_eq!(view.text.as_deref(), Some("draft via ready footer"));
    let rewind = ["› ", "", "\x1b[2mesc again to edit previous message\x1b[0m"].join("\n");
    assert_eq!(
        inspect_codex(&capture(&rewind, (2, 0))).state,
        ComposerState::Empty
    );
    // The same sentence without dim styling is quoted output, not a footer.
    let quoted = ["› old prompt", "", "esc again to edit previous message"].join("\n");
    assert_eq!(
        inspect_codex(&capture(&quoted, (0, 2))).state,
        ComposerState::Unknown
    );
    // Real Codex TUI after a multiline paste: its status bar uses remaining
    // context, with no model/Ready label, and parks the cursor below the text.
    let pasted = [
        "older output",
        "",
        "› Reply with OK.",
        "  continued line",
        "",
        "",
        "tab to queue message                    100% context left",
        "",
    ]
    .join("\n");
    let view = inspect_codex(&capture(&pasted, (2, 4)));
    assert_eq!(view.state, ComposerState::Editing);
    assert!(same_text_ignoring_whitespace(
        view.text.as_deref().unwrap(),
        "Reply with OK. continued line"
    ));
}

#[test]
fn codex_footerless_frame_needs_the_cursor_inside_the_block() {
    let screen = ["older output", "", "› typed on a short pane"].join("\n");
    assert_eq!(
        inspect_codex(&capture(&screen, (10, 2))).state,
        ComposerState::Editing
    );
    // Cursor at the marker column, on a transcript row, or beyond the screen.
    assert_eq!(
        inspect_codex(&capture(&screen, (0, 2))).state,
        ComposerState::Unknown
    );
    assert_eq!(
        inspect_codex(&capture(&screen, (3, 0))).state,
        ComposerState::Unknown
    );
    assert_eq!(
        inspect_codex(&capture(&screen, (3, 9))).state,
        ComposerState::Unknown
    );
    // Cursor parked one or two blank rows above a bottom composer.
    let parked = ["output", "", "› \x1b[2mAsk Codex to do anything\x1b[0m"].join("\n");
    assert_eq!(
        inspect_codex(&capture(
            &["output", "", "", "› bottom row"].join("\n"),
            (2, 1)
        ))
        .state,
        ComposerState::Editing
    );
    assert_eq!(
        inspect_codex(&capture(&parked, (2, 1))).state,
        ComposerState::Empty
    );
    assert_eq!(
        inspect_codex(&capture(&["", "", "", "", "› too far"].join("\n"), (2, 1))).state,
        ComposerState::Unknown
    );
}

#[test]
fn codex_multiline_paste_parks_cursor_below_footerless_editor() {
    // Captured from a real Luna low TUI after a six-line bracketed paste:
    // the model footer vanishes and the cursor sits at column 2 on the first
    // blank row below the editor. SEND must still be able to press Enter.
    let screen = [
        "previous output",
        "",
        "› Reply with OK. Ignore padding.",
        "  xxxxxxxxxxxxxxxxxxxx",
        "  xxxxxxxxxxxxxxxxxxxx",
        "",
        "",
        "",
    ]
    .join("\n");
    let view = inspect_codex(&capture(&screen, (2, 5)));
    assert_eq!(view.state, ComposerState::Editing);
    assert!(view.composer_token.is_some());
    let paragraphs = screen.replace("  xxxxxxxxxxxxxxxxxxxx\n  ", "\n  ");
    assert_eq!(
        inspect_codex(&capture(&paragraphs, (2, 5))).state,
        ComposerState::Editing
    );
    // The cursor position, continuation rows and trailing blank screen are
    // all necessary: a transcript prompt alone never becomes a composer.
    assert_eq!(
        inspect_codex(&capture(&screen, (3, 5))).state,
        ComposerState::Unknown
    );
    assert_eq!(
        inspect_codex(&capture("older output\n\n› old prompt\n", (2, 3))).state,
        ComposerState::Unknown
    );
    let output_below = screen.replace("\n\n\n", "\n\nnew output\n");
    assert_eq!(
        inspect_codex(&capture(&output_below, (2, 5))).state,
        ComposerState::Unknown
    );
}

#[test]
fn codex_menus_lag_and_busy_footer() {
    let menu = "Would you like to run the following command?\n\n  1. Yes (y)\n  2. No (esc)\nPress enter to confirm or esc to cancel";
    assert_eq!(
        inspect_codex(&capture(menu, (0, 4))).state,
        ComposerState::Unknown
    );
    let row = "› draft";
    let mut lagging = capture(&codex_screen(row, CODEX_FOOTER), (7, 4));
    lagging.lag = Some(2);
    let view = inspect_codex(&lagging);
    assert_eq!(view.state, ComposerState::Unknown);
    assert!(view.lagging && view.text.is_none());
    let busy = [
        "• Working (3s • esc to interrupt)",
        "",
        "› ",
        "",
        CODEX_FOOTER,
    ]
    .join("\n");
    let view = inspect_codex(&capture(&busy, (2, 2)));
    assert!(view.busy);
    assert_eq!(view.state, ComposerState::Empty);
    assert!(codex_busy_screen("Working … esc to interrupt"));
    assert!(!codex_busy_screen("plain prompt"));
}
