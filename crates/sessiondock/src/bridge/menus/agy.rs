//! Agy 1.2.16 menus verified in an isolated real PTY. Unverified forms stay
//! in the native terminal; the editor recognizer also rejects their layout.
use crate::delivery::driver::strip_ansi;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn screen_prompt(screen: &str) -> Option<Value> {
    let clean = strip_ansi(screen);
    let lines: Vec<&str> = clean.lines().map(str::trim_end).collect();
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
