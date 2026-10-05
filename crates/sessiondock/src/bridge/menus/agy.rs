//! Agy 1.2.16/1.2.17 menus verified in an isolated real PTY. Unverified forms stay
//! in the native terminal; the editor recognizer also rejects their layout.
use crate::delivery::driver::strip_ansi;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn screen_prompt(screen: &str) -> Option<Value> {
    let clean = strip_ansi(screen);
    let lines: Vec<&str> = clean.lines().map(str::trim_end).collect();
    if let Some(prompt) = write_in_prompt(&clean, &lines) {
        return Some(prompt);
    }
    if let Some(prompt) = interaction_prompt(&clean, &lines) {
        return Some(prompt);
    }
    let footer = lines.iter().rposition(|line| {
        let line = line.trim();
        line.starts_with("Keyboard:") || line == "↑/↓ Navigate · enter Confirm"
    })?;
    // Only the footer/model line can follow the active menu's key hints.
    if lines[footer + 1..]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .count()
        > 1
    {
        return None;
    }
    let (start, kind, labels): (usize, &str, Vec<String>) =
        if lines[footer].trim() == "↑/↓ Navigate · enter Confirm" {
            let question = lines[..footer]
                .iter()
                .rposition(|line| line.trim() == "Do you trust the contents of this project?")?;
            let start = lines[..question]
                .iter()
                .rposition(|line| line.trim() == "Accessing workspace:")?;
            if !lines[question..footer].iter().any(|line| {
                line.trim()
                    == "Antigravity CLI requires permission to read, edit, and execute files here."
            }) {
                return None;
            }
            (
                start,
                "folder_trust",
                vec!["Yes, I trust this folder".into(), "No, exit".into()],
            )
        } else if lines[footer].trim() == "Keyboard: ↑/↓ Navigate  enter Select  esc Go Back" {
            let start = lines[..footer]
                .iter()
                .rposition(|line| line.trim() == "Switch Model")?;
            let rule = (start + 1..footer).find(|&i| lines[i].trim().starts_with("────────"))?;
            let labels = lines[rule + 1..footer]
                .iter()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    line.trim()
                        .strip_prefix("> ")
                        .unwrap_or(line.trim())
                        .to_owned()
                })
                .collect();
            (start, "model", labels)
        } else if lines[footer].trim() == "Keyboard: ↑/↓ Navigate  enter Save  esc Close" {
            let start = lines[..footer]
                .iter()
                .rposition(|line| line.trim() == "Permission Config Editor")?;
            if !lines[start..footer]
                .iter()
                .any(|line| line.trim() == "Select a config scope to edit:")
            {
                return None;
            }
            (
                start,
                "permission_scope",
                vec![
                    "Project".into(),
                    "Shared with Antigravity".into(),
                    "Global".into(),
                ],
            )
        } else {
            return None;
        };
    let selected: Vec<_> = labels
        .iter()
        .enumerate()
        .filter(|(_, label)| {
            lines[start..footer]
                .iter()
                .any(|line| line.trim() == format!("> {label}"))
        })
        .map(|(i, _)| i)
        .collect();
    if selected.len() != 1 || labels.is_empty() {
        return None;
    }
    if labels.iter().any(|label| {
        !lines[start..footer].iter().any(|line| {
            let line = line.trim();
            line == label || line.strip_prefix("> ") == Some(label.as_str())
        })
    }) {
        return None;
    }
    let focus = selected[0];
    let options: Vec<Value> = labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let mut keys = vec![if i < focus { "Up" } else { "Down" }; i.abs_diff(focus)];
            keys.push("Enter");
            json!({"label":label,"keys":keys})
        })
        .collect();
    let question = lines[start..footer]
        .iter()
        .map(|line| line.trim().strip_prefix("> ").unwrap_or(line.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let semantic = format!("{kind}\n{question}");
    let id = format!("{:x}", Sha256::digest(semantic.as_bytes()));
    let revision = format!("{:x}", Sha256::digest(clean.as_bytes()));
    Some(
        json!({"id":format!("agy-screen:{}",&id[..16]),"revision":&revision[..16],
        "source":"agy","kind":"screen_menu","state":"waiting","menu_type":kind,
        "questions":[{"question":question,"options":options,"multiple":false}],
        "cancel_keys":if kind == "folder_trust" { vec![] } else { vec!["Escape"] },"actions":[]}),
    )
}

fn write_in_prompt(clean: &str, lines: &[&str]) -> Option<Value> {
    let footer = lines
        .iter()
        .rposition(|line| line.trim() == "enter Submit · esc Back")?;
    let tail: Vec<_> = lines[footer + 1..]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if tail.len() != 1 || !tail[0].starts_with("esc to cancel") {
        return None;
    }
    let answer = lines[..footer]
        .iter()
        .rposition(|line| line.trim() == "Your answer:")?;
    // Do not overwrite a draft typed directly in the native terminal. Only
    // the visibly empty input has verified append/Enter semantics.
    if lines[answer + 1..footer]
        .iter()
        .any(|line| !line.trim().is_empty())
    {
        return None;
    }
    let start = lines[..answer]
        .iter()
        .rposition(|line| *line == "Question")?;
    if !lines.get(start + 1)?.chars().all(|c| c == '─')
        || !lines[start + 2..answer]
            .iter()
            .any(|line| line.starts_with("Question "))
        || !lines[start + 2..answer]
            .iter()
            .any(|line| line.starts_with("> ") && line.ends_with(". Write-in..."))
    {
        return None;
    }
    let question = lines[start + 2..answer].join("\n").trim().to_owned();
    let id = format!(
        "{:x}",
        Sha256::digest(format!("write_in\n{question}").as_bytes())
    );
    let revision = format!("{:x}", Sha256::digest(clean.as_bytes()));
    Some(
        json!({"id":format!("agy-screen:{}",&id[..16]),"revision":&revision[..16],
        "source":"agy","kind":"screen_menu","state":"waiting","menu_type":"write_in",
        "questions":[{"question":question,"options":[],"multiple":false}],
        "cancel_keys":["Escape"],"actions":[],
        "text":{"label":"Your answer","before_keys":[],"after_keys":["Enter"],
                "multiline":false,"mode":"data","value":""}}),
    )
}

/// Tool approvals and ask_question are native modal pages, not transcript
/// questions. Keep the command/diff and grant scope in the semantic identity.
fn interaction_prompt(clean: &str, lines: &[&str]) -> Option<Value> {
    let footer = lines.iter().rposition(|line| {
        let line = line.trim();
        line.starts_with("↑/↓ Navigate ·")
    })?;
    let tail: Vec<_> = lines[footer + 1..]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if tail.len() != 1 || !tail[0].starts_with("esc to cancel") {
        return None;
    }
    let start = (0..footer).rev().find(|&i| {
        matches!(lines[i], "Command" | "Create file" | "Question")
            && lines
                .get(i + 1)
                .is_some_and(|line| line.len() >= 8 && line.chars().all(|c| c == '─'))
    })?;
    let hints = lines[footer].trim();
    let kind = match lines[start] {
        "Command"
            if hints == "↑/↓ Navigate · tab Amend · ctrl+g edit/expand command"
                && lines[start..footer].contains(&"Requesting permission for:")
                && lines[start..footer].contains(&"Run this command?") =>
        {
            "command_permission"
        }
        "Create file"
            if hints == "↑/↓ Navigate · tab Amend · f full diff"
                && lines[start..footer].contains(&"Allow creation of this file?") =>
        {
            "file_permission"
        }
        "Question"
            if hints.contains(" · esc Skip")
                && (hints.contains(" · enter Select")
                    || hints.contains(" · enter Submit All")
                    || hints.contains(" · enter Next")) =>
        {
            "question"
        }
        _ => return None,
    };
    let multiple = kind == "question" && hints.contains(" · space Toggle");
    let question_end = (start + 2..footer).rev().find(|&i| match kind {
        "command_permission" => lines[i] == "Run this command?",
        "file_permission" => lines[i] == "Allow creation of this file?",
        _ => lines[i].starts_with("Question ") && lines[i].contains(": "),
    })?;
    let mut rows: Vec<(bool, String)> = Vec::new();
    let mut first = None;
    for (i, line) in lines.iter().enumerate().take(footer).skip(question_end + 1) {
        let focused = line.starts_with("> ");
        let text = line.trim().strip_prefix("> ").unwrap_or(line.trim());
        if let Some((number, label)) = text.split_once(". ")
            && number.parse::<usize>().ok() == Some(rows.len() + 1)
        {
            first.get_or_insert(i);
            rows.push((focused, label.to_owned()));
        } else if first.is_some() && !line.trim().is_empty() {
            // Native wrapped labels are indented; don't silently discard a
            // changed widget or turn a text-entry prompt into an approval.
            if !line.starts_with("    ") || text.starts_with('>') {
                return None;
            }
            rows.last_mut()?.1.push('\n');
            rows.last_mut()?.1.push_str(line.trim());
        }
    }
    let first = first?;
    let focus: Vec<_> = rows.iter().enumerate().filter(|(_, row)| row.0).collect();
    if focus.len() != 1 || rows.len() < 2 {
        return None;
    }
    let focus = focus[0].0;
    let mut stable = Vec::new();
    let mut options = Vec::new();
    for (i, (_, label)) in rows.iter().enumerate() {
        let checked = if multiple {
            label
                .strip_prefix("[ ] ")
                .map(|s| (false, s))
                .or_else(|| label.strip_prefix("[x] ").map(|s| (true, s)))
        } else {
            None
        };
        let text = checked.map_or(label.as_str(), |(_, text)| text);
        if multiple && checked.is_none() && text != "Write-in..." {
            return None;
        }
        let mut keys = vec![if i < focus { "Up" } else { "Down" }; i.abs_diff(focus)];
        keys.push(if checked.is_some() { "Space" } else { "Enter" });
        let mut option = json!({"label":text,"keys":keys});
        if let Some((selected, _)) = checked {
            option["toggle"] = json!(true);
            option["selected"] = json!(selected);
        }
        stable.push(text);
        options.push(option);
    }
    let question = lines[start + 2..first].join("\n").trim().to_owned();
    let mut actions = Vec::new();
    if kind == "question" {
        if hints.contains(" · ← Back") {
            actions.push(json!({"label":"Previous question","keys":["Left"]}));
        }
        if hints.contains(" · → Next") {
            actions.push(json!({"label":"Next question","keys":["Right"]}));
        }
        if multiple {
            actions.push(json!({"label":if hints.contains(" · enter Submit All") {
                "Submit All" } else { "Next question" },"keys":["Enter"]}));
        }
    } else {
        actions.push(json!({"label":"Amend","keys":["Tab"]}));
        if kind == "file_permission" {
            actions.push(json!({"label":"Full diff","keys":["f"]}));
        }
    }
    let semantic = format!("{kind}\n{question}\n{}", stable.join("\n"));
    let id = format!("{:x}", Sha256::digest(semantic.as_bytes()));
    let revision = format!("{:x}", Sha256::digest(clean.as_bytes()));
    Some(
        json!({"id":format!("agy-screen:{}",&id[..16]),"revision":&revision[..16],
        "source":"agy","kind":"screen_menu","state":"waiting","menu_type":kind,
        "questions":[{"question":question,"options":options,"multiple":multiple}],
        "cancel_keys":["Escape"],"actions":actions}),
    )
}
