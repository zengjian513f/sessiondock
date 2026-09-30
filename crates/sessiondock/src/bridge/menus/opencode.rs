//! OpenCode v2.0.18 screen menus (official tag cd9a14a6b688d4021bee381dfd39d2cef9c0f862).
//! Read only: answers are native keys chosen by the user after a fresh capture.
//! SGR is essential: ● means the configured value, not keyboard focus.
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

#[derive(Clone, Default, PartialEq, Eq)]
struct Style {
    fg: Option<String>,
    bg: Option<String>,
    bold: bool,
    inverse: bool,
}
#[derive(Clone)]
struct Cell {
    ch: char,
    style: Style,
}
type Row = Vec<Cell>;
static ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[([0-9;]*)([A-Za-z])").unwrap());
static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([1-9][0-9]*)\.\s+(?:\[([ ✓x])\]\s*)?(.+)$").unwrap());
static HINT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(ctrl\+[a-z]|alt\+[a-z]|enter|tab)\s+([A-Za-z][A-Za-z /-]*?)(?:\s{2,}|$)")
        .unwrap()
});

// Captures encode blank runs as CSI C. Keep cell styles while expanding them.
fn rows(screen: &str) -> Vec<Row> {
    let mut style = Style::default();
    screen
        .replace('\r', "")
        .lines()
        .map(|line| {
            let mut row = Vec::new();
            let mut offset = 0;
            for cap in ESCAPE.captures_iter(line) {
                let whole = cap.get(0).unwrap();
                append(&mut row, &line[offset..whole.start()], &style);
                let codes: Vec<u16> = cap[1].split(';').map(|p| p.parse().unwrap_or(0)).collect();
                if &cap[2] == "C" {
                    for _ in 0..codes.first().copied().unwrap_or(1).max(1) {
                        row.push(Cell {
                            ch: ' ',
                            style: style.clone(),
                        });
                    }
                }
                if &cap[2] == "m" {
                    let mut i = 0;
                    while i < codes.len() {
                        match codes[i] {
                            0 => style = Style::default(),
                            1 => style.bold = true,
                            22 => style.bold = false,
                            7 => style.inverse = true,
                            27 => style.inverse = false,
                            39 => style.fg = None,
                            49 => style.bg = None,
                            38 | 48 if codes.get(i + 1) == Some(&2) && i + 4 < codes.len() => {
                                let color = format!(
                                    "rgb:{},{},{}",
                                    codes[i + 2],
                                    codes[i + 3],
                                    codes[i + 4]
                                );
                                if codes[i] == 38 {
                                    style.fg = Some(color)
                                } else {
                                    style.bg = Some(color)
                                };
                                i += 4;
                            }
                            38 | 48 if codes.get(i + 1) == Some(&5) && i + 2 < codes.len() => {
                                let color = format!("index:{}", codes[i + 2]);
                                if codes[i] == 38 {
                                    style.fg = Some(color)
                                } else {
                                    style.bg = Some(color)
                                };
                                i += 2;
                            }
                            n @ (30..=37 | 90..=97) => style.fg = Some(format!("sgr:{n}")),
                            n @ (40..=47 | 100..=107) => style.bg = Some(format!("sgr:{}", n - 10)),
                            _ => {}
                        }
                        i += 1;
                    }
                }
                offset = whole.end();
            }
            append(&mut row, &line[offset..], &style);
            row
        })
        .collect()
}
fn append(row: &mut Row, text: &str, style: &Style) {
    for ch in text.chars().filter(|c| !c.is_control()) {
        row.push(Cell {
            ch,
            style: style.clone(),
        });
        // Terminal columns for CJK/emoji values; continuation cells carry no text.
        if matches!(ch as u32,0x1100..=0x115f|0x2329..=0x232a|0x2e80..=0xa4cf|0xac00..=0xd7a3|0xf900..=0xfaff|0xfe10..=0xfe19|0xfe30..=0xfe6f|0xff00..=0xff60|0xffe0..=0xffe6|0x1f300..=0x1faff|0x20000..=0x3fffd)
        {
            row.push(Cell {
                ch: '\0',
                style: style.clone(),
            });
        }
    }
}
fn text(row: &[Cell]) -> String {
    row.iter()
        .filter(|c| c.ch != '\0')
        .map(|c| c.ch)
        .collect::<String>()
        .trim_end()
        .to_owned()
}
fn first(row: &[Cell]) -> Option<usize> {
    row.iter()
        .position(|c| !c.ch.is_whitespace() && c.ch != '\0')
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))[..16].to_owned()
}
fn payload(
    menu: &str,
    header: &str,
    question: &str,
    options: Vec<Value>,
    multiple: bool,
    revision: &str,
) -> Value {
    json!({"id":format!("opencode-menu:{}",digest(&format!("{menu}\n{header}\n{}", if menu=="text" {String::new()} else {question.replace("[x]", "[]").replace("[✓]", "[]").replace("[ ]", "[]").replace(" ✓", "")}))),"revision":digest(revision),"source":"opencode","kind":"screen_menu","menu_type":menu,"state":"waiting","cancel_keys":["Escape"],"questions":[{"header":header,"question":question,"options":options,"multiple":multiple}]})
}
fn action(label: &str, keys: Vec<String>) -> Value {
    json!({"label":label,"keys":keys})
}
fn option(label: &str, keys: Vec<String>) -> Value {
    json!({"label":label,"keys":keys})
}
fn arrows(current: usize, target: usize, axis: &str) -> Vec<String> {
    let key = if target < current {
        if axis == "horizontal" { "Left" } else { "Up" }
    } else if axis == "horizontal" {
        "Right"
    } else {
        "Down"
    };
    let mut keys = vec![key.to_owned(); current.abs_diff(target)];
    keys.push("Enter".into());
    keys
}
fn content(lines: &[Row]) -> String {
    lines
        .iter()
        .map(|r| text(r).trim().trim_start_matches('┃').trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Only complete live tail panels or styled modal rectangles are recognized.
pub fn screen_prompt(screen: &str) -> Option<Value> {
    let lines = rows(screen);
    panel(&lines, screen).or_else(|| modal(&lines, screen))
}

fn panel(lines: &[Row], screen: &str) -> Option<Value> {
    let footer = lines.iter().rposition(|r| {
        let s = text(r);
        s.trim_start().starts_with('┃')
            && s.contains("enter ")
            && (s.contains("esc ") || s.contains("confirm"))
    })?;
    // A later composer, transcript line, shell prompt or other panel invalidates it.
    if lines[footer + 1..]
        .iter()
        .any(|r| !text(r).trim().is_empty())
    {
        return None;
    }
    let mut start = footer;
    while start > 0 && text(&lines[start - 1]).trim_start().starts_with('┃') {
        start -= 1
    }
    let block = &lines[start..=footer];
    let border_style = block[0].iter().find(|c| c.ch == '┃')?.style.clone();
    if border_style.fg.is_none()
        || border_style.bg.is_none()
        || block.iter().any(|r| {
            r.iter()
                .find(|c| c.ch == '┃')
                .is_none_or(|c| c.style.fg != border_style.fg)
        })
    {
        return None;
    }
    let all = content(block);
    let foot = text(&lines[footer]);
    if all.contains("△ Permission required") {
        let labels = if all.contains("Always allow") {
            vec!["Allow once", "Always allow", "Reject"]
        } else {
            vec!["Allow once", "Reject"]
        };
        let positions: Vec<_> = block
            .iter()
            .enumerate()
            .flat_map(|(row, r)| {
                labels
                    .iter()
                    .enumerate()
                    .filter_map(move |(i, label)| find(r, label).map(|col| (row, col, i)))
            })
            .collect();
        if positions.len() != labels.len() || !foot.contains("confirm") {
            return None;
        }
        let border = block.iter().find_map(|r| {
            r.iter()
                .find(|c| c.ch == '┃')
                .and_then(|c| c.style.fg.clone())
        });
        let selected = positions.iter().find_map(|(r, c, i)| {
            let s = &block[*r][*c].style;
            (s.inverse || (border.is_some() && s.bg == border)).then_some(*i)
        })?;
        let question = content(&block[..positions.iter().map(|p| p.0).min()?]);
        let opts = labels
            .iter()
            .enumerate()
            .map(|(i, label)| option(label, arrows(selected, i, "horizontal")))
            .collect();
        let mut p = payload(
            "permission",
            "Permission required",
            &question,
            opts,
            false,
            screen,
        );
        // Expanded permission Escape minimizes before rejecting; never shortcut rejection.
        p["cancel_keys"] = json!([]);
        p["actions"] = json!([]);
        return Some(p);
    }
    if all.contains("△ Reject permission")
        && all.contains("Tell OpenCode what to do differently")
        && foot.contains("esc cancel")
    {
        let mut p = payload(
            "permission_rejection",
            "Reject permission",
            &content(&block[..block.len() - 1]),
            vec![],
            false,
            screen,
        );
        let value = text(&block[block.len() - 1])
            .split("enter confirm")
            .next()
            .unwrap_or("")
            .trim()
            .trim_start_matches('┃')
            .trim()
            .to_owned();
        p["text"] = json!({"label":"Tell OpenCode what to do differently","value":value,"before_keys":if value.is_empty(){Vec::<String>::new()}else{vec!["Ctrl+c".to_owned()]},"after_keys":["Enter"],"multiline":false,"mode":"paste"});
        return Some(p);
    }
    if all.contains("Session location unavailable")
        && all.contains("Choose directory")
        && foot.contains("confirm")
    {
        let mut p = payload(
            "session_recovery",
            "Session location unavailable",
            &content(&block[..block.len() - 1]),
            vec![option("Choose directory", vec!["Enter".into()])],
            false,
            screen,
        );
        p["cancel_keys"] = json!([]);
        return Some(p);
    }
    if !foot.contains("esc dismiss") && !foot.contains("esc close") {
        return None;
    }
    let title = block
        .iter()
        .map(|r| text(r).trim().trim_start_matches('┃').trim().to_owned())
        .find(|s| !s.is_empty())?;
    let indexed: Vec<_> = block[..block.len() - 1]
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let s = text(r);
            let s = s.trim().trim_start_matches('┃').trim();
            NUMBER.captures(s).map(|c| {
                (
                    i,
                    c[1].parse::<usize>().unwrap(),
                    c.get(2).map(|m| m.as_str().to_owned()),
                    c[3].trim().trim_end_matches(" ✓").to_owned(),
                )
            })
        })
        .collect();
    let multiple = indexed.iter().any(|x| x.2.is_some());
    let editing = foot.contains("esc close");
    let question = content(&block[..indexed.first().map_or(block.len() - 1, |x| x.0)]);
    let panel_bg = block.iter().find_map(|r| {
        r.iter()
            .find(|c| c.ch != '┃' && !c.ch.is_whitespace())
            .and_then(|c| c.style.bg.clone())
    });
    let focused_number = indexed.iter().find_map(|(r, n, _, _)| {
        let c = block[*r].iter().find(|c| c.ch.is_ascii_digit())?;
        (c.style.bg != panel_bg).then_some(*n)
    });
    let mut opts = Vec::new();
    let mut custom = None;
    for (position, (row, n, checked, label)) in indexed.iter().enumerate() {
        if *n > 9 && focused_number.is_none() {
            continue;
        } // Above 9 requires a fresh styled focus.
        if label == "Type your own answer" || editing && position + 1 == indexed.len() {
            custom = Some((*n, label.clone()));
            continue;
        }
        let mut o = option(
            label,
            if *n <= 9 {
                vec![n.to_string()]
            } else {
                arrows(focused_number?, *n, "vertical")
            },
        );
        if multiple {
            o["toggle"] = json!(true);
            o["selected"] =
                json!(checked.as_deref() == Some("✓") || checked.as_deref() == Some("x"));
        }
        let end = indexed.get(position + 1).map_or(block.len() - 1, |x| x.0);
        let desc = content(&block[row + 1..end]);
        if !desc.is_empty() {
            o["description"] = json!(desc)
        }
        opts.push(o)
    }
    let mut p = payload(
        if multiple { "form_multiple" } else { "form" },
        &title,
        &question,
        opts,
        multiple,
        screen,
    );
    let mut actions = Vec::new();
    if !indexed.is_empty() && !editing {
        actions.push(action("Previous answer", vec!["Up".into()]));
        actions.push(action("Next answer", vec!["Down".into()]));
    }
    if foot.contains("⇆ tab") {
        actions.push(action("Next field", vec!["Tab".into()]));
        if !editing && !indexed.is_empty() {
            actions.push(action("Previous field", vec!["Left".into()]));
        }
    }
    if indexed.is_empty() && foot.contains("enter submit") {
        p["menu_type"] = json!("form_review");
        actions.push(action("Submit", vec!["Enter".into()]));
    } else if foot.contains("open link") || foot.contains("I finished") || foot.contains("continue")
    {
        let label = if foot.contains("open link") {
            "Open link"
        } else if foot.contains("I finished") {
            "I finished"
        } else {
            "Continue"
        };
        actions.push(action(label, vec!["Enter".into()]));
        if foot.contains("c copy") {
            actions.push(action("Copy link", vec!["c".into()]));
        }
    } else if let Some((n, _)) = custom {
        let value = if editing {
            indexed
                .last()
                .map(|x| x.3.as_str())
                .filter(|v| *v != "Type your own answer")
                .unwrap_or("")
        } else {
            ""
        };
        p["text"] = json!({"label":"Type your own answer","value":value,"before_keys":if editing{if value.is_empty(){Vec::<String>::new()}else{vec!["Ctrl+c".into()]}}else{vec![n.to_string()]},"after_keys":["Enter"],"multiline":false,"mode":"paste"});
    } else if indexed.is_empty() && (foot.contains("enter confirm") || foot.contains("enter done"))
    {
        let raw = block[..block.len() - 1]
            .iter()
            .rev()
            .map(|r| text(r).trim().trim_start_matches('┃').trim().to_owned())
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let placeholder = [
            "Type your answer",
            "name@example.com",
            "https://example.com",
            "YYYY-MM-DD",
            "YYYY-MM-DDTHH:MM:SSZ",
        ]
        .contains(&raw.as_str())
            || raw.starts_with("at least ")
            || raw.starts_with("at most ");
        let value = if placeholder { String::new() } else { raw };
        p["text"] = json!({"label":"Answer","value":value,"before_keys":if value.is_empty(){Vec::<String>::new()}else{vec!["Ctrl+c".to_owned()]},"after_keys":["Enter"],"multiline":false,"mode":"paste"});
    }
    if editing {
        p["cancel_keys"] = json!([]);
        actions.push(action("Close answer edit", vec!["Escape".into()]));
    }
    p["actions"] = json!(actions);
    Some(p)
}
fn find(row: &Row, needle: &str) -> Option<usize> {
    let target: Vec<_> = needle.chars().collect();
    (0..row.len()).find(|&i| {
        row[i..]
            .iter()
            .filter(|c| c.ch != '\0')
            .map(|c| c.ch)
            .take(target.len())
            .eq(target.iter().copied())
    })
}

fn modal(lines: &[Row], screen: &str) -> Option<Value> {
    // A genuine modal has a bold header and an esc label on its own rectangle.
    let (header_row, left, right, title_col, header) =
        lines.iter().enumerate().rev().find_map(|(n, row)| {
            let esc = find(row, "esc")?;
            let first = (0..esc)
                .find(|&i| row[i].style.bold && !row[i].ch.is_whitespace() && row[i].ch != '\0')?;
            if row[first].style.bg.is_none() || row[first].style.bg != row[esc].style.bg {
                return None;
            }
            let bg = &row[first].style.bg;
            let mut left = first;
            while left > 0 && row[left - 1].style.bg == *bg {
                left -= 1
            }
            let mut right = esc + 3;
            while right < row.len() && row[right].style.bg == *bg {
                right += 1
            }
            let header = text(&row[first..esc]).trim().to_owned();
            if header.is_empty() {
                return None;
            }
            Some((n, left, right, first, header))
        })?;
    let bg = lines[header_row][title_col].style.bg.clone();
    let fg = lines[header_row][title_col].style.fg.clone();
    let muted = find(&lines[header_row], "esc")
        .map(|i| lines[header_row][i].style.fg.clone())
        .flatten();
    let mut end = header_row + 1;
    while end < lines.len() && lines[end].get(left).is_some_and(|c| c.style.bg == bg) {
        end += 1
    }
    if end <= header_row + 1 {
        return None;
    }
    let body: Vec<Row> = lines[header_row + 1..end]
        .iter()
        .map(|r| r[left.min(r.len())..right.min(r.len())].to_vec())
        .collect();
    let question = content(&body);
    let base = title_col - left;
    // Authentication material stays in the terminal, including masked native fields.
    if question.contains("API key") || question.contains("Authorization code") || header == "Pair" {
        return None;
    }
    let nonblank: Vec<_> = body.iter().filter(|r| first(r).is_some()).collect();
    let last = nonblank.last().map(|r| text(r)).unwrap_or_default();
    if header == "File Changes Found" || header == "Delete worktree?" {
        if !last.contains("no") || !last.contains("yes") {
            return None;
        }
        return Some(payload(
            "confirmation",
            &header,
            &question,
            vec![
                option("no", vec!["Left".into(), "Enter".into()]),
                option("yes", vec!["Right".into(), "Enter".into()]),
            ],
            false,
            screen,
        ));
    }
    if last.trim() == "Close" && header == "Session exported" || last.trim() == "ok" {
        return Some(payload(
            "alert",
            &header,
            &question,
            vec![option(
                if header == "Session exported" {
                    "Close"
                } else {
                    "ok"
                },
                vec!["Enter".into()],
            )],
            false,
            screen,
        ));
    }
    for labels in [
        ["Cancel", "Confirm"],
        ["Skip", "Update"],
        ["Skip", "Restart"],
    ] {
        if labels.iter().all(|label| last.contains(label)) {
            let row = *nonblank.last()?;
            let selected = labels
                .iter()
                .enumerate()
                .filter_map(|(i, label)| find(row, label).map(|c| (i, &row[c].style)))
                .find_map(|(i, s)| (s.inverse || s.bg != bg).then_some(i))?;
            return Some(payload(
                "confirmation",
                &header,
                &question,
                labels
                    .iter()
                    .enumerate()
                    .map(|(i, label)| option(label, arrows(selected, i, "horizontal")))
                    .collect(),
                false,
                screen,
            ));
        }
    }
    // Text prompts have a visible submit footer; source defines these as one line.
    if nonblank.last().is_some_and(|r| text(r).contains("submit"))
        && (header == "Rename session"
            || header == "Rename account"
            || header == "Name worktree"
            || header.ends_with(" / New worktree"))
    {
        let mut p = payload("text", &header, &question, vec![], false, screen);
        let value = nonblank
            .get(nonblank.len().saturating_sub(2))
            .map(|r| text(r).trim().to_owned())
            .unwrap_or_default();
        let value = if ["Enter text", "Worktree name"].contains(&value.as_str()) {
            String::new()
        } else {
            value
        };
        p["text"] = json!({"label":header,"value":value,"before_keys":["Ctrl+a","Ctrl+k"],"after_keys":["Enter"],"multiline":false,"mode":"paste"});
        if header == "Name worktree" {
            p["actions"] = json!([action("Generate one", vec!["Tab".into()])])
        }
        return Some(p);
    }
    if header == "Export session" {
        return None;
    } // Focus colors overlap saved values: source does not expose unambiguous active state.
    let mut items: Vec<(usize, String, Style)> = Vec::new();
    let mut selected = None;
    for (n, row) in body.iter().enumerate() {
        let Some(col) = first(row) else { continue };
        let s = &row[col].style;
        let label = text(row).trim().trim_start_matches('●').trim().to_owned();
        if label == "Search" || label == "No items available" || label == "No results found" {
            continue;
        }
        let active = (s.bold && s.bg != bg) || s.inverse;
        let title_style = row
            .iter()
            .enumerate()
            .skip(base)
            .take(4)
            .find(|(_, c)| !c.ch.is_whitespace() && c.ch != '\0')
            .map(|(_, c)| &c.style)
            .unwrap_or(s);
        let normal = col >= base.saturating_sub(2)
            && col <= base + 3
            && (!title_style.bold)
            && title_style.fg != muted
            && (title_style.fg == fg || label.starts_with('[') || row.iter().any(|c| c.ch == '●'));
        if active || normal {
            if active {
                if selected.is_some() {
                    return None;
                }
                selected = Some(items.len());
            }
            items.push((n, label, s.clone()));
        }
    }
    // Modal search text is not a selectable row. Its row precedes the first group/option.
    let selected = selected?;
    if items.is_empty() {
        return None;
    }
    let mut opts = Vec::new();
    let multiple = items
        .iter()
        .any(|(_, label, _)| label.starts_with("[x]") || label.starts_with("[ ]"));
    for (i, (_, label, _)) in items.iter().enumerate() {
        let mut o = option(label, arrows(selected, i, "vertical"));
        if multiple && (label.starts_with("[x]") || label.starts_with("[ ]")) {
            o["toggle"] = json!(true);
            o["selected"] = json!(label.starts_with("[x]"));
            o["label"] = json!(label[3..].trim());
        }
        opts.push(o)
    }
    let mut p = payload("select", &header, &question, opts, multiple, screen);
    if header != "Working directory" && header != "Pending steer" {
        let query = body
            .iter()
            .take(items[0].0)
            .filter_map(|r| first(r).map(|i| (r, i)))
            .find(|(r, i)| r[*i].style.fg != muted && !r[*i].style.bold)
            .map(|(r, _)| text(r).trim().to_owned())
            .unwrap_or_default();
        p["text"] = json!({"label":"Search","value":query,"before_keys":["Ctrl+a","Ctrl+k"],"after_keys":[],"multiline":false,"mode":"data"});
    }
    let mut actions = vec![
        action("Previous item", vec!["Up".into()]),
        action("Next item", vec!["Down".into()]),
        action("Previous page", vec!["PageUp".into()]),
        action("Next page", vec!["PageDown".into()]),
        action("First item", vec!["Home".into()]),
        action("Last item", vec!["End".into()]),
    ];
    for cap in HINT.captures_iter(&last) {
        let key = if cap[1].starts_with("ctrl+") {
            format!("Ctrl+{}", &cap[1][5..])
        } else if &cap[1] == "enter" {
            "Enter".into()
        } else if &cap[1] == "tab" {
            "Tab".into()
        } else {
            continue;
        };
        actions.push(action(cap[2].trim(), vec![key]));
    }
    // Settings/experiments left/right changes the selected value, Enter may open a subpicker.
    if header == "Settings" || header == "Experiments" {
        actions.push(action("Previous value", vec!["Left".into()]));
        actions.push(action("Next value", vec!["Right".into()]));
    }
    p["actions"] = json!(actions);
    Some(p)
}
