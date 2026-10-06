//! Claude Code 2.1.285's live selection surfaces.
//!
//! The installed executable embeds the exact source. The shared Select,
//! MultiSelect and Confirm controls are the behavioral authority; see the
//! versioned inventory fixture for source offsets and unsupported surfaces.
//! This module only describes keys. It never writes or chooses a default.

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:❯\s*)?(\d+)\.\s*(.*)$").expect("Claude option"));
static ACCESSIBLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^Select with numbers \[1-(\d+)\].*Then .*Enter to (?:submit|Submit|Next|confirm).*$",
    )
    .expect("Claude accessible selection")
});
static TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^Enter text for option (\d+) \((.+)\), or Escape for the list: ?(.*)$")
        .expect("Claude accessible text")
});

#[derive(Clone)]
struct Row {
    line: usize,
    number: Option<usize>,
    label: String,
    description: String,
    focused: bool,
    selected: bool,
    checkbox: bool,
    disabled: bool,
}

fn border(line: &str) -> bool {
    let s = line.trim();
    !s.is_empty() && s.chars().all(|c| "─━═╭╮╰╯┌┐└┘┬┴├┤┼│┃ ".contains(c))
}

fn title(line: &str) -> bool {
    let s = line.trim().trim_matches(|c| "│┃╭╮─ ".contains(c));
    [
        "Accessing workspace:",
        "Managed settings require approval",
        "Tool use",
        "Bash command",
        "PowerShell command",
        "Edit file",
        "Write file",
        "Overwrite file",
        "Read file",
        "Read outside the working directories",
        "Fetch",
        "WebFetch",
        "Enter plan mode",
        "Exit plan mode",
        "Permission",
        "Rewind",
        "Review your answers",
        "Select model",
        "Select effort",
        "Select a project",
        "Select a session",
        "Resume session",
        "Settings",
        "Sandbox",
        "Choose the text style",
        "Select login method:",
        "How do you want to sign in?",
        "Log in to Claude",
        "Allow external CLAUDE.md",
        "Now in a new directory:",
        "Moving to a new directory:",
        "WARNING:",
        "Working directory has changes",
        "Enable ",
        "Install ",
        "Add marketplace?",
        "Remove ",
        "Delete ",
        "Trust this ",
        "Connect cloud sessions",
        "Let cloud sessions",
        "Use this machine",
        "Remote Control",
        "Claude in Chrome",
        "Plugins",
        "Manage MCP servers",
        "Select remote environment",
        "What do you want to do?",
        "What would you like to do?",
        "Run ultrareview",
        "Background this session?",
        "Teach auto mode",
        "Confirm switching the model",
    ]
    .iter()
    .any(|p| s.starts_with(p))
}

fn row(line: usize, raw: &str) -> Option<Row> {
    let s = raw.trim();
    if s.starts_with(['>', '|', '`']) {
        return None;
    }
    let focused = s.starts_with('❯');
    let number = NUMBER.captures(s);
    let (index, mut label) = if let Some(ref captures) = number {
        (captures[1].parse().ok(), captures[2].to_owned())
    } else if focused {
        (None, s.trim_start_matches('❯').trim().to_owned())
    } else {
        return None;
    };
    let disabled = label.contains("(disabled)");
    let mut selected =
        label.contains("(selected)") || label.ends_with(" ✓") || label.ends_with(" ✔");
    label = label
        .replace("(selected)", "")
        .replace("(disabled)", "")
        .trim()
        .to_owned();
    let checkbox = label.starts_with("[ ]")
        || label.starts_with("[✓]")
        || label.starts_with("[✔]")
        || label.starts_with("[x]")
        || label.starts_with("[X]");
    if checkbox {
        selected |= !label.starts_with("[ ]");
        label = label.get(label.find(']')? + 1..)?.trim().to_owned();
    }
    label = label
        .trim_end_matches(" ✓")
        .trim_end_matches(" ✔")
        .trim()
        .to_owned();
    let (label, description) = label
        .split_once(" — ")
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
        .unwrap_or((label, String::new()));
    if label.is_empty() {
        return None;
    }
    Some(Row {
        line,
        number: index,
        label,
        description,
        focused,
        selected,
        checkbox,
        disabled,
    })
}

fn navigation(from: usize, to: usize, confirm: bool) -> Vec<String> {
    let key = if to > from { "Down" } else { "Up" };
    let mut keys = vec![key.to_owned(); to.abs_diff(from)];
    if confirm {
        keys.push("Enter".to_owned());
    }
    keys
}

fn digest(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))[..16].to_owned()
}

fn credential_field(context: &str) -> bool {
    context.lines().any(|line| {
        let s = line.trim().to_lowercase();
        s.starts_with("password:")
            || s.starts_with("api key:")
            || s.starts_with("token:")
            || s.starts_with("secret:")
            || s.contains("enter password")
            || s.contains("paste your code")
            || s.contains("authentication code:")
    })
}

/// Ink may hard-wrap a single native guide across capture rows. Reassemble
/// only the static guide: a wrapped editable buffer has ambiguous whitespace
/// and remains in the terminal. This also prevents an old guide in history
/// from consuming subsequent editor/output rows.
fn current_guide(lines: &[&str], end: usize) -> Option<(usize, String)> {
    let prefix = |line: &str| {
        let s = line.trim();
        s.starts_with("Select with numbers [1-")
            || s.starts_with("Enter text for option ")
            || s.starts_with("Enter y/n:")
            || s.starts_with("Enter to ")
            || s.starts_with("Esc to ")
    };
    let start = (0..=end)
        .rfind(|&i| {
            let s = lines[i].trim();
            s.starts_with("Select with numbers [1-")
                || s.starts_with("Enter text for option ")
                || s.starts_with("Enter y/n:")
        })
        .or_else(|| (0..=end).rfind(|&i| prefix(lines[i])))?;
    if start == end {
        return Some((start, lines[start].trim().to_owned()));
    }
    let parts: Vec<&str> = lines[start..=end].iter().map(|s| s.trim()).collect();
    if parts
        .iter()
        .any(|s| s.is_empty() || s.starts_with(['>', '|', '`', '❯']) || border(s))
    {
        return None;
    }
    // The editable value follows the final ":" in the source's generated
    // accessibility guide. Once it starts, a line break is not reversible.
    if parts[..parts.len() - 1]
        .iter()
        .any(|s| s.contains(": ") || s.ends_with(':'))
    {
        return None;
    }
    for joined in [parts.concat(), parts.join(" ")] {
        if joined.starts_with("Select with numbers [1-") {
            if ACCESSIBLE.is_match(&joined)
                && joined.rsplit_once(':').is_some_and(|(_, v)| {
                    v.chars()
                        .all(|c| c.is_ascii_digit() || c == ',' || c == ' ')
                })
            {
                return Some((start, joined));
            }
        } else if joined.starts_with("Enter text for option ") {
            if TEXT.is_match(&joined) {
                return Some((start, joined));
            }
        } else if joined.starts_with("Enter y/n:") {
            // Its prefix is short; any wrapping necessarily involves its
            // editable value, which is intentionally not reconstructed.
            return None;
        } else {
            let lower = joined.to_lowercase();
            let native_hint = |s: &str| {
                s.contains("to cancel")
                    || s.contains("to close")
                    || s.contains("to exit")
                    || s.contains("to go back")
                    || s.contains("to confirm")
                    || s.contains("to select")
                    || s.contains("to navigate")
                    || s.contains("to add notes")
                    || s.contains("tab/arrow keys to navigate")
            };
            if (lower.contains("esc to ") || lower.contains("escape to "))
                && parts.iter().skip(1).all(|s| native_hint(&s.to_lowercase()))
            {
                return Some((start, joined));
            }
        }
    }
    None
}

fn payload(question: String, menu_type: &str, options: Vec<Value>, multiple: bool) -> Value {
    let semantic = json!({"question": question, "menu_type": menu_type,
        "labels": options.iter().map(|v| &v["label"]).collect::<Vec<_>>()});
    json!({"id": format!("claude-screen:{}", digest(&semantic.to_string())),
        "source":"claude", "kind":"screen_menu", "state":"waiting", "menu_type":menu_type,
        "cancel_keys":["Escape"], "questions":[{"header":"Claude Code", "question":question,
        "multiple":multiple, "options":options}]})
}

/// Parse only the final complete native menu frame. Cursor location is fresh
/// state, not part of the semantic ID. Keys are recomputed on every capture.
pub fn screen_prompt(screen: &str) -> Option<Value> {
    let clean = crate::delivery::driver::strip_ansi(screen).replace('\r', "");
    let lines: Vec<&str> = clean.lines().map(str::trim_end).collect();
    let tail = lines
        .iter()
        .rposition(|s| !s.trim().is_empty() && !border(s))?;
    let mut footer = tail;
    // Native question AFK timers are advisory and follow the input guide.
    if lines[footer].trim().starts_with("Auto-continuing in ") {
        footer = (0..footer).rfind(|&i| !lines[i].trim().is_empty() && !border(lines[i]))?;
    }
    let original_guide = lines[footer].trim();
    let (guide_start, guide_text) =
        current_guide(&lines, footer).unwrap_or((footer, original_guide.to_owned()));
    footer = guide_start;
    let guide = guide_text.as_str();
    if guide.starts_with(['>', '|', '`']) {
        return None;
    }
    let lower = guide.to_lowercase();
    // Onboarding deliberately hides the normal input guide. Its seven theme
    // rows and final syntax-preview caption form a distinct complete frame.
    if guide.starts_with("Syntax theme:") {
        let start = lines.iter().rposition(|s| {
            s.trim() == "Choose the text style that looks best with your terminal"
        })?;
        let theme_rows: Vec<Row> = (start..footer).filter_map(|i| row(i, lines[i])).collect();
        let expected = [
            "Auto (match terminal)",
            "Dark mode",
            "Light mode",
            "Dark mode (colorblind-friendly)",
            "Light mode (colorblind-friendly)",
            "Dark mode (ANSI colors only)",
            "Light mode (ANSI colors only)",
        ];
        if theme_rows.len() != 7
            || theme_rows
                .iter()
                .zip(expected)
                .any(|(r, label)| r.label != label)
            || !lines[start..footer]
                .iter()
                .any(|s| s.contains("Hello, Claude!"))
        {
            return None;
        }
        return screen_prompt(&format!("{clean}\nEnter to select · Esc to cancel"));
    }
    if let Some(start) = lines
        .iter()
        .rposition(|s| s.trim() == "Select login method:")
    {
        let login_rows: Vec<Row> = (start..=footer).filter_map(|i| row(i, lines[i])).collect();
        if login_rows.len() >= 2
            && login_rows.len() <= 3
            && login_rows.last().is_some_and(|r| r.line == footer)
            && login_rows.iter().all(|r| {
                r.label.starts_with("Claude account with subscription")
                    || r.label.starts_with("Anthropic Console account")
                    || r.label.starts_with("3rd-party platform")
            })
        {
            return screen_prompt(&format!("{clean}\nEnter to select · Esc to cancel"));
        }
    }
    let accessible = ACCESSIBLE.captures(guide);
    let yes_no = guide.starts_with("Enter y/n:");
    let text = TEXT.captures(guide);
    let normal_guide = (lower.contains("esc to cancel")
        || lower.contains("esc to close")
        || lower.contains("esc to exit")
        || lower.contains("esc to go back"))
        && (lower.contains("enter to ")
            || lower.contains("tab to ")
            || lower == "esc to cancel"
            || lower.contains("add notes")
            || lower.contains("navigate"));
    if accessible.is_none() && !yes_no && text.is_none() && !normal_guide {
        return None;
    }
    if yes_no || text.is_some() {
        let start = (0..footer)
            .rfind(|&i| title(lines[i]) || border(lines[i]))
            .map(|i| if border(lines[i]) { i + 1 } else { i })
            .or_else(|| (0..footer).rfind(|&i| lines[i].trim().ends_with('?')))?;
        if lines[start..footer]
            .iter()
            .any(|s| s.trim().starts_with('>') || s.trim().starts_with("```"))
        {
            return None;
        }
        if lines[..start]
            .iter()
            .filter(|s| s.trim().starts_with("```"))
            .count()
            % 2
            != 0
        {
            return None;
        }
        let context = lines[start..footer].join("\n");
        if credential_field(&context) {
            return None;
        }
        if let Some(captures) = text {
            let mut value = payload(context.clone(), "text", vec![], false);
            value["text"] = json!({"label":captures[2].to_owned(),
                "before_keys":vec!["Backspace";captures[3].encode_utf16().count()],
                "after_keys":["Enter"],"multiline":false,"mode":"data","value":captures[3].to_owned()});
            value["context"] = json!(context);
            value["actions"] = json!([{"label":"Back to choices","keys":["Escape"]}]);
            return Some(value);
        }
        let yes = lines[start..footer]
            .iter()
            .find_map(|s| s.trim().strip_prefix("y. "))?;
        let no = lines[start..footer]
            .iter()
            .find_map(|s| s.trim().strip_prefix("n. "))?;
        let typed = guide.strip_prefix("Enter y/n:")?.trim_start();
        let keys = |letter: &str| {
            let mut keys = vec!["Backspace".to_owned(); typed.encode_utf16().count()];
            keys.extend([letter.to_owned(), "Enter".to_owned()]);
            keys
        };
        return Some(payload(
            context,
            "confirmation",
            vec![
                json!({"label":yes,"keys":keys("y")}),
                json!({"label":no,"keys":keys("n")}),
            ],
            false,
        ));
    }
    let mut rows: Vec<Row> = (0..footer).filter_map(|i| row(i, lines[i])).collect();
    let mut anchor_index = rows
        .iter()
        .rposition(|r| r.focused)
        .or_else(|| rows.len().checked_sub(1))?;
    while anchor_index > 0
        && rows[anchor_index - 1]
            .number
            .is_some_and(|number| rows[anchor_index].number == Some(number + 1))
    {
        anchor_index -= 1;
    }
    let anchor = rows[anchor_index].line;
    // A single AskUserQuestion hides the arrows and Submit tab, and uses the
    // Up/Down guide. Its checkbox header is still navigation, not question
    // text; including it breaks reconciliation with the native/hook question.
    let question_tabs = lower
        .contains("navigate")
        .then(|| {
            (0..anchor).rfind(|&i| {
                let s = lines[i].trim();
                (s.starts_with('←') && s.contains("Submit") && s.ends_with('→'))
                    || (s.starts_with(['☐', '☑', '☒'])
                        && lines.get(i + 1).is_some_and(|line| line.trim().is_empty()))
            })
        })
        .flatten();
    let start = question_tabs.map(|i| i + 1).or_else(|| {
        (0..anchor)
            .rfind(|&i| title(lines[i]) || border(lines[i]))
            .map(|i| if border(lines[i]) { i + 1 } else { i })
            .or_else(|| (0..anchor).rfind(|&i| lines[i].trim().ends_with('?')))
            .or_else(|| {
                (0..anchor).rfind(|&i| !lines[i].trim().is_empty() && row(i, lines[i]).is_none())
            })
    })?;
    if lines[start..=footer].iter().any(|s| {
        let t = s.trim();
        t.starts_with('>') || t.starts_with("```")
    }) {
        return None;
    }
    if lines[..start]
        .iter()
        .filter(|s| s.trim().starts_with("```"))
        .count()
        % 2
        != 0
    {
        return None;
    }
    rows.retain(|r| r.line >= start);
    // Unnumbered Confirm rows share indentation; collect only the short,
    // adjacent list. Trust never guesses an absent confirmation label.
    if rows.iter().all(|r| r.number.is_none()) && rows.len() == 1 {
        let focus = rows[0].line;
        let indent = lines[focus].find('❯')?;
        for (i, line) in lines.iter().enumerate().take(footer).skip(start) {
            if i == focus || line.trim().is_empty() || border(line) {
                continue;
            }
            let s = line.trim();
            if s.len() > 250 {
                continue;
            }
            let leading = line.len() - line.trim_start().len();
            if leading >= indent + 2
                && (s.starts_with("Yes")
                    || s.starts_with("No")
                    || s == "Cancel"
                    || s == "Submit answers")
            {
                rows.push(Row {
                    line: i,
                    number: None,
                    label: s.to_owned(),
                    description: String::new(),
                    focused: false,
                    selected: false,
                    checkbox: false,
                    disabled: false,
                });
            }
        }
        rows.sort_by_key(|r| r.line);
    }
    // Native Select renders descriptions on indented continuation rows. Keep
    // them with their option rather than folding every non-option into the
    // question (including all descriptions of a multi-question form).
    let mut description_lines = std::collections::HashSet::new();
    let option_lines: std::collections::HashSet<_> = rows.iter().map(|r| r.line).collect();
    for r in &mut rows {
        let leading = lines[r.line]
            .chars()
            .take_while(|c| c.is_whitespace())
            .count();
        for (i, line) in lines.iter().enumerate().take(footer).skip(r.line + 1) {
            let indent = line.chars().take_while(|c| c.is_whitespace()).count();
            if line.trim().is_empty()
                || border(line)
                || option_lines.contains(&i)
                || indent < leading + 2
            {
                break;
            }
            if !r.description.is_empty() {
                r.description.push('\n');
            }
            r.description.push_str(line.trim());
            description_lines.insert(i);
        }
    }
    let question = (start..footer)
        .filter(|i| !rows.iter().any(|r| r.line == *i))
        .filter(|i| !description_lines.contains(i) && !border(lines[*i]))
        .filter(|&i| !matches!(lines[i].trim(), "Submit" | "Next"))
        .map(|i| lines[i].trim())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    if question.is_empty() {
        return None;
    }
    if credential_field(&question) {
        return None;
    }
    let count = rows.iter().filter(|r| r.focused).count();
    let is_multi = rows.iter().any(|r| r.checkbox)
        || lower.contains("space to toggle")
        || lower.contains("comma- or space-separated");
    let focused = rows.iter().position(|r| r.focused);
    let submit = rows
        .iter()
        .position(|r| matches!(r.label.as_str(), "Submit" | "Next"));
    if accessible.is_none() && !yes_no && text.is_none() && count != 1 {
        return None;
    }
    if rows.iter().any(|r| r.number.is_some()) {
        let mut prev = None;
        for r in rows.iter().filter(|r| r.number.is_some()) {
            if prev.is_some_and(|p| r.number != Some(p + 1)) {
                return None;
            }
            prev = r.number;
        }
    }
    if let Some(ref captures) = accessible {
        let total: usize = captures[1].parse().ok()?;
        if rows.iter().filter(|r| r.number.is_some()).count() != total {
            return None;
        }
    }
    let typed = if accessible.is_some() {
        guide.rsplit_once(": ").map(|(_, s)| s).unwrap_or("")
    } else {
        ""
    };
    if is_multi && accessible.is_some() && !typed.is_empty() {
        let values: Vec<usize> = typed
            .split([',', ' '])
            .filter_map(|s| s.parse().ok())
            .collect();
        for r in &mut rows {
            r.selected = r.number.is_some_and(|n| values.contains(&n));
        }
    }
    let mut options = Vec::new();
    let mut text_field = None;
    for (i, r) in rows.iter().enumerate() {
        if r.disabled || submit == Some(i) {
            continue;
        }
        let input = r.label == "Other"
            || r.label.starts_with("Type something")
            || r.label.starts_with("No, keep planning")
            || r.label.starts_with("Tell Claude what")
            || r.label.starts_with("Summarize from here")
            || r.label.starts_with("Summarize up to here");
        if input && accessible.is_none() {
            text_field = Some(
                json!({"label":r.label,"before_keys":navigation(focused?,i,false),
                "after_keys":["Enter"],"multiline":false,"mode":"paste"}),
            );
            continue;
        }
        let keys = if accessible.is_some() {
            let n = r.number?;
            let mut keys = vec!["Backspace".to_owned(); typed.encode_utf16().count()];
            if is_multi {
                let mut chosen: Vec<usize> = rows
                    .iter()
                    .filter(|r| r.selected)
                    .filter_map(|r| r.number)
                    .collect();
                if chosen.contains(&n) {
                    chosen.retain(|v| *v != n);
                } else {
                    chosen.push(n);
                }
                chosen.sort_unstable();
                if chosen.is_empty() {
                    // A numeric form with defaults cannot express an empty
                    // selection: bare Enter restores those defaults.
                    continue;
                }
                keys.extend(
                    chosen
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                        .chars()
                        .map(|c| c.to_string()),
                );
            } else {
                keys.extend(n.to_string().chars().map(|c| c.to_string()));
                keys.push("Enter".to_owned());
            }
            keys
        } else if is_multi && r.number.is_some_and(|n| n < 10) {
            vec![r.number?.to_string()]
        } else {
            navigation(focused?, i, !is_multi)
        };
        let keys = if is_multi && accessible.is_none() && r.number.is_none() {
            let mut keys = keys;
            keys.push("Space".to_owned());
            keys
        } else {
            keys
        };
        let mut option = json!({"label":r.label,"keys":keys});
        if !r.description.is_empty() {
            option["description"] = json!(r.description);
        }
        if is_multi {
            option["selected"] = json!(r.selected);
            option["toggle"] = json!(true);
        }
        options.push(option);
    }
    // Never expose a one-button approval when the other native row was lost.
    if options.len() < 2 && text_field.is_none() && !is_multi {
        return None;
    }
    let mut result = payload(
        question,
        if is_multi { "multi_select" } else { "select" },
        options,
        is_multi,
    );
    if let Some(field) = text_field {
        result["text"] = field;
    }
    if lower.contains("tab/arrow keys to navigate") {
        result["actions"] = json!([{"label":"Previous question","keys":["Left"]},
            {"label":"Next question / review","keys":["Right"]}]);
    }
    if is_multi && accessible.is_some() {
        result["actions"] = json!([{"label":"Submit / Next","keys":["Enter"]}]);
    }
    if is_multi && accessible.is_none() {
        let visible_submit = submit.or_else(|| {
            (rows.last()?.line + 1..footer)
                .find(|&i| matches!(lines[i].trim(), "Submit" | "Next"))
                .map(|_| rows.len())
        });
        if let Some(target) = visible_submit {
            let mut keys = navigation(focused?, target, false);
            keys.push("Enter".to_owned());
            let label = submit
                .map(|i| rows[i].label.as_str())
                .unwrap_or("Submit / Next");
            let action = json!({"label":label,"keys":keys});
            if let Some(actions) = result["actions"].as_array_mut() {
                actions.push(action);
            } else {
                result["actions"] = json!([action]);
            }
        }
    }
    // Revision excludes focus but tracks checkbox/text state for review.
    result["revision"] = json!(digest(
        &json!({"selected":rows.iter().map(|r|r.selected).collect::<Vec<_>>(),
        "text":result.get("text")})
        .to_string()
    ));
    // Descriptions rendered on later lines remain visible in the full context.
    result["context"] = json!(lines[start..footer].join("\n"));
    Some(result)
}
