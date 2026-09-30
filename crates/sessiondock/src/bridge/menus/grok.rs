//! Native Grok Build screen menus. Provenance and exceptions are in the menu inventory.
//! Selected answer markers are not keyboard focus; multi-select uses the bold
//! label rendered by question_view, never the checkbox's selected state.
use crate::delivery::driver::strip_ansi;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

static ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([1-9a-z])\s+(?:\(([●•○])\)|\[([ xX])\])\s+(.+)$").unwrap());
static SGR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[([0-9;]*)m").unwrap());
static PAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([0-9]+)/([0-9]+)\]").unwrap());
fn hash(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))[..16].into()
}
fn content(s: &str) -> &str {
    s.trim()
        .trim_start_matches(['┃', '│'])
        .trim()
        .trim_end_matches('│')
        .trim()
}
fn bold_label(raw: &str, label: &str) -> bool {
    let mut bold = false;
    let mut spans = Vec::new();
    let mut pos = 0;
    for cap in SGR.captures_iter(raw) {
        let m = cap.get(0).unwrap();
        spans.extend(raw[pos..m.start()].chars().map(|c| (c, bold)));
        let mut codes = cap[1].split(';');
        while let Some(code) = codes.next() {
            match code {
                "" | "0" | "22" => bold = false,
                "1" => bold = true,
                // RGB/indexed color parameters are not SGR attributes.
                "38" | "48" | "58" => match codes.next() {
                    Some("2") => {
                        codes.next();
                        codes.next();
                        codes.next();
                    }
                    Some("5") => {
                        codes.next();
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        pos = m.end();
    }
    spans.extend(raw[pos..].chars().map(|c| (c, bold)));
    let text: String = spans.iter().map(|(c, _)| c).collect();
    let Some(offset) = text.find(label) else {
        return false;
    };
    let prefix = text[..offset].chars().count();
    spans[prefix..]
        .iter()
        .take(label.chars().count())
        .filter(|(c, _)| !c.is_whitespace())
        .all(|(_, b)| *b)
}
fn move_to(from: usize, to: usize, last: &str) -> Vec<String> {
    let mut keys = vec![
        if from > to {
            "Up".into()
        } else {
            "Down".into()
        };
        from.abs_diff(to)
    ];
    keys.push(last.into());
    keys
}
fn payload(
    question: &str,
    semantic: &str,
    kind: &str,
    options: Vec<Value>,
    cancel: Vec<&str>,
    actions: Vec<Value>,
) -> Value {
    json!({"id":format!("grok-screen:{}",hash(semantic)),"revision":hash(question),"source":"grok","kind":"screen_menu","state":"waiting","menu_type":kind,"questions":[{"question":question,"options":options,"multiple":kind=="multi_select"}],"cancel_keys":cancel,"actions":actions})
}

pub fn screen_prompt(screen: &str) -> Option<Value> {
    let raw: Vec<&str> = screen.lines().collect();
    let clean = strip_ansi(screen).replace('\r', "");
    let lines: Vec<&str> = clean.lines().collect();
    let end = lines.iter().rposition(|l| !l.trim().is_empty())?;
    if lines[..=end]
        .iter()
        .any(|l| l.trim_start().starts_with("```") || l.trim_start().starts_with("> "))
    {
        return None;
    }
    // Welcome trust uses its own unbordered renderer and direct y/n handlers.
    if let Some(start) = lines[..=end]
        .iter()
        .rposition(|l| l.trim() == "Do you trust the contents of this directory?")
    {
        let block = &lines[start..=end];
        let visible = block
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join("\n");
        if !visible.contains("Grok Build may run or modify contents in this directory,")
            || !visible.contains("posing security risks.")
        {
            return None;
        }
        let yes = block.iter().position(|l| {
            l.trim() == "Yes, proceed    y"
                || l.trim().starts_with("Yes, proceed") && l.trim().ends_with('y')
        })?;
        let no = block
            .iter()
            .position(|l| l.trim().starts_with("No, quit") && l.trim().ends_with('n'))?;
        if yes >= no
            || block[no + 1..].iter().any(|l| {
                !l.trim().is_empty()
                    && !l.trim().starts_with("1.0.41")
                    && !l.trim().starts_with("Grok Build")
            })
        {
            return None;
        }
        let question = block[..=no]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join("\n");
        return Some(payload(
            &question,
            &question,
            "folder_trust",
            vec![
                json!({"label":"Yes, proceed","keys":["y"]}),
                json!({"label":"No, quit","keys":["n"]}),
            ],
            vec!["Escape"],
            vec![],
        ));
    }
    let tail = content(lines[end]);
    if tail.starts_with("Save changes?") || tail.starts_with("Save and send?") {
        let sending = tail.starts_with("Save and send?");
        let labels = if sending {
            ["save & send", "discard & send", "delete prompt"]
        } else {
            ["save", "discard changes", "delete prompt"]
        };
        let mut options = Vec::new();
        for (key, label) in ["y", "n", "x"].iter().zip(labels) {
            if !tail.contains(&format!("{key}:{label}")) {
                return None;
            }
            options.push(json!({"label":label,"keys":[key]}));
        }
        return Some(payload(
            tail,
            tail,
            "confirmation",
            options,
            vec!["Escape"],
            vec![],
        ));
    }
    // The live shortcuts bar must terminate the captured frame.
    let question_footer =
        tail.contains("next answer") && tail.contains("Tab") && tail.contains("Esc");
    let permission_footer = tail.contains("select")
        && tail.contains("always-approve")
        && tail.contains("cancel")
        && tail.contains("Ctrl");
    let cancel_turn_footer = tail.contains("next choice") && tail.contains("confirm");
    let rewind_footer = (tail.contains("select") || tail.contains("confirm"))
        && tail.contains("Esc")
        && !tail.contains("next answer");
    let text_footer = tail.contains("submit") && tail.contains("Enter") && tail.contains("Esc");
    let pattern_footer = tail.contains("save")
        && tail.contains("Esc")
        && (tail.contains("Enter") || tail.contains("enter"));
    if !(question_footer
        || permission_footer
        || cancel_turn_footer
        || rewind_footer
        || pattern_footer
        || text_footer)
    {
        return picker_prompt(&lines, &raw, end);
    }
    let last_card = lines[..end]
        .iter()
        .rposition(|l| l.trim_start().starts_with(['┃', '│']))?;
    let mut start = last_card;
    while start > 0 {
        let previous = lines[start - 1].trim();
        if previous.starts_with(['┃', '│'])
            || previous.is_empty()
            || previous.contains("↑/↓ navigate") && previous.contains("Enter:")
        {
            start -= 1;
        } else {
            break;
        }
    }
    let block = &lines[start..=end];
    let contents: Vec<&str> = block.iter().map(|l| content(l)).collect();
    if contents.iter().any(|l| {
        l.contains("…")
            || l.contains("... ")
            || l.contains("[selected lines unavailable]")
            || l.contains("[plan content unavailable]")
    }) {
        return None;
    }
    let question = contents.join("\n");
    let mut rows = Vec::new();
    for (index, line) in contents.iter().enumerate() {
        if let Some(c) = ROW.captures(line) {
            let text = c.get(4)?.as_str();
            rows.push((
                index,
                c[1].to_owned(),
                c.get(3).map(|v| v.as_str().eq_ignore_ascii_case("x")),
                text.to_owned(),
            ));
        }
    }
    // The command-pattern editor has an explicit clear() binding (Ctrl-U).
    if rows.is_empty() && pattern_footer && question.contains("command pattern") {
        let input = contents.iter().find(|l| l.starts_with('❯'))?;
        let value = input.trim_start_matches('❯').trim();
        let stable = contents
            .iter()
            .map(|l| {
                if l.starts_with('❯') {
                    "❯ <pattern>"
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut p = payload(&question, &stable, "text", vec![], vec!["Escape"], vec![]);
        p["text"] = json!({"label":"Command pattern","before_keys":["ctrl-u"],"after_keys":["Enter"],"multiline":false,"mode":"data","value":value});
        return Some(p);
    }
    if text_footer && !question_footer {
        let empty = rows.iter().find(|r| r.1 == "z" && r.3.trim() == "❯")?;
        if contents[empty.0 + 1..].iter().any(|l| {
            !l.is_empty() && !l.contains("navigate") && !l.contains("Enter:") && !l.contains("Esc:")
        }) {
            return None;
        }
        let semantic = contents[..empty.0].join("\n");
        let mut p = payload(&question, &semantic, "text", vec![], vec!["Escape"], vec![]);
        p["revision"] = json!(hash(screen));
        p["text"] = json!({"label":"Answer","before_keys":[],"after_keys":["Enter"],"multiline":true,"mode":"paste","value":""});
        return Some(p);
    }
    if rows.is_empty() {
        return None;
    }
    let multi = rows.iter().any(|r| r.2.is_some());
    if multi && !question_footer {
        return None;
    }
    let focused = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            bold_label(
                raw.get(start + r.0).copied().unwrap_or(""),
                r.3.split("  ").next().unwrap_or(&r.3),
            )
        })
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let cursor = if multi {
        if focused.len() != 1 {
            return None;
        }
        focused[0]
    } else {
        0
    };
    let mut options = Vec::new();
    let mut stable_lines = contents.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    let mut free = None;
    for (i, row) in rows.iter().enumerate() {
        let (line, key, checked, text) = row;
        let label = text.split("  ").next()?.trim();
        if key == "z" && question_footer {
            // Native drafts are deliberately retained in PTY. Only the literal
            // placeholder proves the hidden freeform draft is empty.
            if label == "Type your answer here" {
                free = Some(i);
            }
            stable_lines[*line] = "z <freeform>".into();
            continue;
        }
        let keys = if multi {
            move_to(cursor, i, "Space")
        } else if permission_footer || cancel_turn_footer {
            if !key.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            vec![key.clone()]
        } else if question_footer {
            if !key.chars().all(|c| matches!(c,'1'..='9'|'a'..='f')) {
                return None;
            }
            vec![key.clone()]
        } else if question.starts_with("\nRewind")
            || question.contains("Would you like to cancel it before rewinding?")
        {
            vec![key.clone()]
        } else {
            return None;
        };
        let mut option = json!({"label":label,"keys":keys});
        let next = rows.get(i + 1).map_or(contents.len() - 1, |r| r.0);
        let mut desc = text.strip_prefix(label)?.trim().to_string();
        for extra in &contents[line + 1..next] {
            if !extra.is_empty() && !extra.contains("navigate") && !extra.contains("Enter:") {
                if !desc.is_empty() {
                    desc.push('\n');
                }
                desc.push_str(extra);
            }
        }
        if !desc.is_empty() {
            option["description"] = json!(desc);
        }
        if let Some(checked) = checked {
            option["toggle"] = json!(true);
            option["selected"] = json!(checked);
        }
        stable_lines[*line] = format!("{} {}", key, text);
        options.push(option);
    }
    let mut actions = vec![
        json!({"label":"Previous options","keys":["Up"]}),
        json!({"label":"Next options","keys":["Down"]}),
    ];
    let mut cancel = vec!["Escape"];
    if question_footer {
        if tail.contains("dismiss") {
            cancel = vec!["X"];
        }
        if let Some(page) = PAGE.captures(&question) {
            if &page[2] != "1" {
                actions.push(json!({"label":"Previous question","keys":["Left"]}));
                actions.push(json!({"label":"Next question","keys":["Right"]}));
            }
        }
        if multi {
            if let Some((i, _)) = rows
                .iter()
                .enumerate()
                .find(|(_, r)| r.2 == Some(true) && r.1 != "z")
            {
                actions.push(json!({"label":if question.contains("Enter:select"){"Next question with current choices"}else{"Submit current choices"},"keys":move_to(cursor,i,"Enter")}));
            }
        }
        if free.is_some() {
            actions.push(json!({"label":"Type another answer","keys":["z"]}));
        }
        actions.push(json!({"label":"Expand question","keys":["ctrl-f"]}));
    }
    if permission_footer {
        cancel = vec!["ctrl-c"];
        if question.contains("narrow scope") {
            actions.push(json!({"label":"Narrow permission scope","keys":["Left"]}));
            actions.push(json!({"label":"Widen permission scope","keys":["Right"]}));
        }
        if question.contains("edit pattern") {
            actions.push(json!({"label":"Edit permission pattern","keys":["e"]}));
        }
    }
    let semantic = stable_lines
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            // Only renderer-owned unbordered footer rows are focus chrome.
            // Commands and warnings retain these words even when they quote keys.
            *i + 1 < stable_lines.len()
                && (block[*i].trim_start().starts_with(['┃', '│'])
                    || !content(block[*i]).contains("↑/↓ navigate")
                    || !content(block[*i]).contains("Enter:"))
        })
        .map(|(_, l)| l.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let semantic = if let Some(page) = PAGE.captures(&question) {
        format!("{semantic}\n{}", &page[0])
    } else {
        semantic
    };
    let mut p = payload(
        &question,
        &semantic,
        if multi {
            "multi_select"
        } else if permission_footer {
            "approval"
        } else if question_footer {
            "request_user_input"
        } else {
            "confirmation"
        },
        options,
        cancel,
        actions,
    );
    p["revision"] = json!(hash(screen));
    Some(p)
}

fn picker_prompt(lines: &[&str], raw: &[&str], end: usize) -> Option<Value> {
    // Shared ModalWindow + picker chrome: complete bottom border and footer.
    let bottom = lines[..=end]
        .iter()
        .rposition(|l| l.trim_start().starts_with('╰') && l.trim_end().ends_with('╯'))?;
    if lines[bottom + 1..=end].iter().any(|l| !l.trim().is_empty()) {
        return None;
    }
    let top = lines[..bottom]
        .iter()
        .rposition(|l| l.trim_start().starts_with('╭') && l.trim_end().ends_with('╮'))?;
    let block = &lines[top..=bottom];
    let text = block
        .iter()
        .map(|l| content(l))
        .collect::<Vec<_>>()
        .join("\n");
    if text.contains("Settings")
        && text.contains("Reset '")
        && text.contains("y reset")
        && text.contains("n cancel")
        && text.contains("Esc cancel")
    {
        return Some(payload(
            &text,
            &text,
            "confirmation",
            vec![
                json!({"label":"Reset to default","keys":["y"]}),
                json!({"label":"Cancel","keys":["n"]}),
            ],
            vec!["Escape"],
            vec![],
        ));
    }
    if text.contains("Settings ›")
        && text.contains("↑/↓")
        && text.contains("←/→")
        && (text.contains("+/-") || text.contains("step"))
        && text.contains("Enter commit")
        && text.contains("Esc cancel")
    {
        return Some(payload(
            &text,
            &text,
            "setting",
            vec![],
            vec!["Escape"],
            vec![
                json!({"label":"Increase by one step","keys":["Up"]}),
                json!({"label":"Decrease by one step","keys":["Down"]}),
                json!({"label":"Increase by a larger step","keys":["Right"]}),
                json!({"label":"Decrease by a larger step","keys":["Left"]}),
                json!({"label":"Commit current value","keys":["Enter"]}),
            ],
        ));
    }
    if !text.contains("Enter select")
        || !(text.contains("Esc close")
            || text.contains("Esc cancel")
            || text.contains("Esc revert"))
    {
        return None;
    }
    if ![
        "Commands",
        "Pick model",
        "Pick reasoning effort",
        "Pick theme",
        "Pick option",
        "Settings ›",
    ]
    .iter()
    .any(|title| text.contains(title))
    {
        return None;
    }
    if text.contains('…') {
        return None;
    }
    let mut entries = Vec::new();
    for (offset, line) in block.iter().enumerate() {
        let c = content(line);
        if let Some(label) = c
            .strip_prefix("◆ ")
            .or_else(|| c.strip_prefix("› "))
            .or_else(|| {
                if text.contains("Settings ›") {
                    c.strip_prefix("●  ")
                        .or_else(|| c.strip_prefix("•  "))
                        .or_else(|| c.strip_prefix("○  "))
                } else {
                    None
                }
            })
        {
            entries.push((offset, label.to_owned()));
        }
    }
    let cursors = entries
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            bold_label(
                raw.get(top + r.0).copied().unwrap_or(""),
                r.1.split("  ").next().unwrap_or(&r.1),
            )
        })
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    if cursors.len() != 1 {
        return None;
    }
    let options = entries
        .iter()
        .enumerate()
        .map(|(i, (_, entry))| {
            let (label, desc) = entry
                .split_once("  ")
                .or_else(|| entry.split_once(" · "))
                .unwrap_or((entry, ""));
            let mut o = json!({"label":label.trim(),"keys":move_to(cursors[0],i,"Enter")});
            if !desc.trim().is_empty() {
                o["description"] = json!(desc.trim());
            }
            o
        })
        .collect();
    let semantic = text.replace("●  ", "○  ").replace("•  ", "○  ");
    let mut p = payload(
        &text,
        &semantic,
        "selection",
        options,
        vec!["Escape"],
        vec![
            json!({"label":"Previous options","keys":["Up"]}),
            json!({"label":"Next options","keys":["Down"]}),
        ],
    );
    p["revision"] = json!(hash(&raw.join("\n")));
    Some(p)
}
