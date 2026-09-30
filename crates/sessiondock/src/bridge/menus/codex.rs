//! Screen-only Codex menus, audited against the installed rust-v0.159.2 release.
//! This module only describes explicit terminal actions; it never answers a menu.
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::delivery::driver::strip_ansi;

static ROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?P<cursor>[›>])?\s*(?P<number>[1-9][0-9]*)\.\s+(?P<text>.+)$").unwrap()
});
static CHECKBOX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?P<cursor>›)?\s*\[(?P<checked>[ xX])\]\s+(?P<text>.+)$").unwrap()
});
static PAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(Question|Field) ([0-9]+)/([0-9]+)(?: \([^)]*\))?(?: · auto-resolves in [0-9ms ]+)?$",
    )
    .unwrap()
});
static SHORTCUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\((y|p|a|r|d|x|n|c|esc)\)\s*$").unwrap());
static MEMORY_ACTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?P<cursor>›)?\s*(?P<text>(?:Reset all memories|Go back)(?:\s{2,}.*)?)$")
        .unwrap()
});
static KEYMAP_ROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?P<cursor>›)?\s*(?P<text>(?:Global|Chat|Composer|Editor|List|App|Agents|Vim normal|Vim insert)\s{2,}.+)$").unwrap()
});
static DISABLED_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:–\s+)?(?P<text>\S.*\(disabled\).*)$").unwrap());

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))[..16].to_owned()
}

fn heading(line: &str) -> bool {
    matches!(
        line,
        "Folder access"
            | "Submit with unanswered questions?"
            | "Hooks need review"
            | "Select Model"
            | "Select Model and Effort"
            | "Advanced Reasoning"
            | "Update Model Permissions"
            | "Select Approval Mode"
            | "How was this?"
            | "Select Syntax Theme"
            | "Select voice"
            | "Select Pet"
            | "Skills"
            | "Apps"
            | "Plugins"
            | "Worktrees"
            | "Managed worktrees"
            | "Delete this worktree?"
            | "Archive this session?"
            | "Delete this session?"
            | "Resume paused goal?"
            | "Replace goal?"
            | "Task is still running"
            | "Implement this plan?"
            | "Select a review preset"
            | "Select a base branch"
            | "Select a commit to review"
            | "Copy to clipboard"
            | "Export conversation"
            | "Approaching rate limits"
            | "Usage"
            | "Usage limit resets"
            | "Use this reset?"
            | "Chat paused as a precaution"
            | "TUI mode for next launch"
            | "Select an open-source provider"
            | "Experimental features"
            | "Configure Status Line"
            | "Configure Terminal Title"
            | "Enable/Disable Skills"
            | "Choose how you want to use Codex."
            | "Subagents"
            | "Agents"
            | "Stop this attempt and retry?"
            | "Choose what happens to the current task."
            | "Enable full access?"
            | "Auto-review Denials"
            | "Daemon"
            | "Update daemon and exit Codex?"
            | "Apply reasoning change"
            | "Background server has incompatible feature settings"
            | "Cannot use the background server"
            | "Shared agents unavailable"
            | "Unable to resume session"
            | "Memories"
            | "Reset all memories?"
            | "Keymap"
            | "Edit Shortcut"
            | "Replace Binding"
            | "Shortcut Conflict"
            | "Giving this request a little extra thought"
    ) || PAGE.is_match(line)
        || line.starts_with("Would you like to ")
        || line.starts_with("Do you want to approve network access to ")
        || line.starts_with("Select Reasoning Level for ")
        || line.starts_with("Select a reasoning level")
        || line.starts_with("Update available · ")
        || line.starts_with("✨") && line.contains("Would you like to update?")
        || line.starts_with("Sign in with ChatGPT to use Codex")
        || line.starts_with("Your organization requires the default Codex agent sandbox")
        || line.starts_with("Couldn't set up your sandbox")
        || line.starts_with("You’re now using Luna,")
        || line.starts_with("Archive “") && line.ends_with("”?")
        || line.starts_with("Permanently delete “") && line.ends_with("”?")
        || line.starts_with("Enable ") && line.ends_with('?')
        || line.starts_with("▌ Tell us more (")
        || line == "> Set up Amazon Bedrock"
}

fn key_hint(text: &str) -> Option<String> {
    let key = text.trim().replace("ctrl + ", "ctrl+");
    let key = key.replace('+', "-");
    match key.as_str() {
        "esc" => Some("Escape".into()),
        "enter" => Some("Enter".into()),
        "tab" => Some("Tab".into()),
        "↑" => Some("Up".into()),
        "↓" => Some("Down".into()),
        "←" => Some("Left".into()),
        "→" => Some("Right".into()),
        _ if key.len() == 1 && key.is_ascii() => Some(key),
        _ if key.starts_with("ctrl-") && key.len() == 6 => Some(key),
        _ if key.starts_with('f')
            && key[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            Some(key)
        }
        _ => None,
    }
}

fn footer(text: &str) -> bool {
    static FIXED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
        r"^(?:Press enter to confirm or esc to (?:cancel(?: or o to open thread)?|go back|continue working)",
        r"|Press enter to submit or esc to cancel|Press enter to continue(?: Press esc to go back)?",
        r"|enter (?:select|apply|default|continue(?: and create sandbox)?|confirm)(?: · s session)? · esc (?:back|quit|skip|close)",
        r"|space toggle · (?:←/→ reorder · )?enter save · esc (?:cancel|save/close)",
        r"|space toggle · enter save/select · esc cancel",
        r"|left/right group · enter edit shortcut · \* custom · - unbound · esc close",
        r"|enter start inspector · esc close",
        r"|No action is required\. Codex will keep waiting, and this menu will close when the response is ready\.",
        r"|space/enter toggle · esc close|↑/↓ choose · enter select · esc LM Studio · ctrl\+c exit)$"
    )).unwrap()
    });
    if FIXED.is_match(text) {
        return true;
    }
    static FORM_SEGMENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"^(?:option [0-9]+/[0-9]+|tab to add notes|tab or esc to clear notes",
            r"|(?:enter|ctrl\+[a-z]|f[0-9]+) to submit(?: answer| all)?",
            r"|←/→ to navigate questions|ctrl\+p / ctrl\+n change question",
            r"|(?:esc|ctrl\+[a-z]|f[0-9]+) to (?:interrupt|cancel))$"
        ))
        .unwrap()
    });
    text.contains(" to submit")
        && text
            .split('|')
            .all(|segment| FORM_SEGMENT.is_match(segment.trim()))
}

#[derive(Debug)]
struct Choice {
    number: String,
    text: String,
    cursor: bool,
    checked: Option<bool>,
    line: usize,
}

fn movement(from: usize, to: usize, final_key: &str) -> Vec<String> {
    let mut keys = vec![
        if to < from {
            "Up".into()
        } else {
            "Down".into()
        };
        from.abs_diff(to)
    ];
    keys.push(final_key.into());
    keys
}

/// Only a complete, current menu at the capture tail is eligible.
pub fn screen_prompt(screen: &str) -> Option<Value> {
    let clean = strip_ansi(screen).replace('\r', "");
    let lines: Vec<&str> = clean.lines().map(str::trim_end).collect();
    let end = lines.iter().rposition(|line| !line.trim().is_empty())?;
    // A copied fenced menu and line-quoted transcripts are never actionable.
    if lines[..=end]
        .iter()
        .filter(|line| line.trim_start().starts_with("```"))
        .count()
        % 2
        != 0
    {
        return None;
    }
    // User-verification headings are server-defined. Its native Server field,
    // verification/cancel rows, and exact approval footer establish this layout.
    let server = lines[..=end]
        .iter()
        .rposition(|line| line.trim().starts_with("Server: "));
    let verification = server.filter(|_| {
        lines[..=end]
            .iter()
            .any(|line| line.contains("Verify and approve (y)"))
            && lines[..=end]
                .iter()
                .any(|line| line.contains("Cancel this request (c)"))
    });
    let start = if let Some(server) = verification {
        let mut start = server;
        while start > 0 && lines[start - 1].trim().is_empty() {
            start -= 1;
        }
        if start > 0 && lines[start - 1].trim().starts_with("Thread: ") {
            start -= 1;
            while start > 0 && lines[start - 1].trim().is_empty() {
                start -= 1;
            }
        }
        while start > 0 && !lines[start - 1].trim().is_empty() {
            start -= 1;
        }
        start
    } else {
        lines[..=end]
            .iter()
            .rposition(|line| heading(line.trim()))?
    };
    let block = &lines[start..=end];
    if block
        .iter()
        .any(|line| line.trim_start().starts_with("```"))
    {
        return None;
    }
    // Omitted disclosures/commands cannot be reconstructed from a clipped capture.
    if block
        .iter()
        .any(|line| line.contains("[… ") || line.contains(" lines]"))
    {
        return None;
    }
    // Footer text can wrap across terminal rows. It must consume the screen tail.
    let foot = (0..block.len())
        .rev()
        .find(|index| {
            let joined = block[*index..]
                .iter()
                .map(|line| line.trim())
                .collect::<Vec<_>>()
                .join(" ");
            footer(&joined)
        })
        .or_else(|| {
            // These source-defined selectors intentionally render no key-hint footer.
            // Their final native row is fixed; later output invalidates the tail.
            let title = block[0].trim();
            let last = ROW.captures(block.last()?)?.name("text")?.as_str().trim();
            if matches!(
                title,
                "Background server has incompatible feature settings"
                    | "Cannot use the background server"
            ) && last == "Cancel"
                || title == "Stop this attempt and retry?" && last == "Stop and retry"
            {
                Some(block.len())
            } else {
                None
            }
        })?;
    let foot_text = block[foot..]
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join(" ");
    let title = block[0].trim();
    let page = PAGE.captures(title);
    let is_form = page.is_some();
    let is_question = title.starts_with("Question ");
    let is_approval = verification.is_some()
        || title.starts_with("Would you like to ")
        || title.starts_with("Do you want to approve network access to ");
    let is_memories = title == "Memories";
    let is_memory_reset = title == "Reset all memories?";
    let is_keymap = title == "Keymap";
    let is_checkbox = is_memories
        || matches!(
            title,
            "Experimental features"
                | "Configure Status Line"
                | "Configure Terminal Title"
                | "Enable/Disable Skills"
        );
    let mut choices: Vec<Choice> = Vec::new();
    for (index, line) in block[..foot].iter().enumerate().skip(1) {
        let row = if is_checkbox {
            CHECKBOX.captures(line).or_else(|| {
                if is_memories {
                    MEMORY_ACTION.captures(line)
                } else {
                    None
                }
            })
        } else if is_memory_reset {
            MEMORY_ACTION.captures(line)
        } else if is_keymap {
            KEYMAP_ROW.captures(line)
        } else {
            ROW.captures(line).or_else(|| DISABLED_ROW.captures(line))
        };
        if let Some(row) = row {
            choices.push(Choice {
                number: row
                    .name("number")
                    .map_or(String::new(), |v| v.as_str().into()),
                text: row.name("text")?.as_str().trim().into(),
                cursor: row.name("cursor").is_some(),
                checked: row
                    .name("checked")
                    .map(|v| v.as_str().eq_ignore_ascii_case("x")),
                line: index,
            });
        }
    }
    // Notes are a different keyboard focus; numbered shortcuts would type into it.
    let notes = block[..foot]
        .iter()
        .position(|line| line.trim().starts_with("› Add notes"));
    let free = block[..foot]
        .iter()
        .position(|line| line.trim().starts_with("› Type your answer"))
        .or_else(|| {
            if is_question && choices.is_empty() {
                block[..foot]
                    .iter()
                    .position(|line| line.trim().starts_with('›'))
            } else {
                None
            }
        });
    let text_only = free.is_some() || notes.is_some() || title.starts_with("▌ Tell us more (");
    // MCP does not disclose whether an empty text field's schema is secret.
    if title.starts_with("Field ") && text_only {
        return None;
    }
    if title.starts_with("▌ Tell us more (") {
        // Feedback uses a border shared by disclosure and input. Only its visible
        // empty-input placeholder proves that pasting will not append to a draft.
        let placeholder = block[..foot]
            .iter()
            .rposition(|line| line.trim().starts_with("▌ (optional)"))?;
        if block[placeholder + 1..foot]
            .iter()
            .any(|line| !line.trim().is_empty())
        {
            return None;
        }
    }
    if choices.is_empty() && !text_only {
        return None;
    }
    if !text_only && choices.iter().filter(|row| row.cursor).count() != 1 {
        return None;
    }
    if !is_checkbox
        && !is_memory_reset
        && !is_keymap
        && choices
            .iter()
            .any(|row| row.number.parse::<usize>().is_err() && !row.text.contains("(disabled)"))
    {
        return None;
    }
    let selected = choices.iter().position(|row| row.cursor).unwrap_or(0);
    let mut options = Vec::new();
    for (index, choice) in choices.iter().enumerate() {
        // Keep continuation text, paths, disabled entries, and disclosures intact.
        let next = choices.get(index + 1).map_or(foot, |row| row.line);
        let mut raw = choice.text.clone();
        for line in &block[choice.line + 1..next] {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('›') && line != "↓" && line != "↑" {
                raw.push('\n');
                raw.push_str(line);
            }
        }
        let disabled = raw.contains("(disabled)");
        let parts = raw.split_once("  ");
        let label = parts.map_or(raw.as_str(), |(label, _)| label).trim();
        let mut keys = if disabled {
            Vec::new()
        } else if is_memories && choice.checked.is_none() || is_memory_reset || is_keymap {
            movement(selected, index, "Enter")
        } else if is_checkbox {
            movement(selected, index, "Space")
        } else if is_approval {
            let folded = raw.replace('\n', " ");
            let shortcut = SHORTCUT.captures(&folded)?;
            vec![key_hint(shortcut.get(1)?.as_str())?]
        } else if title == "Folder access" {
            if choice.number == "1" {
                vec!["1".into(), "Enter".into()]
            } else {
                vec!["2".into()]
            }
        } else if title == "Submit with unanswered questions?" {
            vec![choice.number.clone(), "Enter".into()]
        } else if block[..foot]
            .iter()
            .any(|line| line.trim().starts_with("Type to search"))
        {
            movement(selected, index, "Enter")
        } else if choice.number.parse::<usize>().ok()? <= 9 {
            vec![choice.number.clone()]
        } else {
            movement(selected, index, "Enter")
        };
        if title == "Hooks need review" && label.starts_with("Trust all")
            || title == "TUI mode for next launch"
            || title == "Chat paused as a precaution" && index == 0
            || title == "Update daemon and exit Codex?" && index == 1
            || title == "Background server has incompatible feature settings"
                && label == "Restart with these settings"
            || title.starts_with("Permanently delete “") && index == 1
            || title == "Stop this attempt and retry?" && label == "Stop and retry"
            || label.contains("without sandbox")
        {
            keys.push("Enter".into());
        }
        if notes.is_some() {
            keys.insert(0, "Tab".into());
        }
        let mut option = json!({"label":label,"keys":if disabled {vec![]} else {keys}});
        if disabled {
            option["disabled"] = json!(true);
        }
        if let Some((_, description)) = parts {
            option["description"] = json!(description.trim());
        }
        if let Some(checked) = choice.checked {
            option["selected"] = json!(checked);
            option["toggle"] = json!(true);
        }
        options.push(option);
    }
    let question = block[..foot]
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n");
    let stable = block[..foot]
        .iter()
        .map(|line| {
            if let Some(page) = PAGE.captures(line.trim()) {
                return format!("{} {}/{}", &page[1], &page[2], &page[3]);
            }
            line.trim()
                .trim_start_matches('›')
                .trim_start_matches('>')
                .trim()
                .replace("[x]", "[ ]")
                .replace("[X]", "[ ]")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut prompt = json!({
        "id":format!("codex-screen:{}",digest(&stable)), "source":"codex",
        "kind":"screen_menu", "state":"waiting", "revision":digest(&question),
        "menu_type":if is_checkbox { "multi_select" } else if is_form { "request_user_input" } else if is_approval { "approval" } else { "selection" },
        "questions":[{"header":title,"question":question,"options":options,"multiple":is_checkbox}],
    });
    let cancel = if foot_text.contains("esc LM Studio") || notes.is_some() {
        "ctrl-c".into()
    } else if let Some((prefix, _)) = foot_text.rsplit_once(" to interrupt") {
        key_hint(prefix.rsplit('|').next()?.trim())?
    } else {
        "Escape".into()
    };
    prompt["cancel_keys"] = json!([cancel]);
    let mut actions: Vec<Value> = Vec::new();
    if title == "Chat paused as a precaution" {
        prompt["cancel_keys"] = json!([]);
    }
    if is_checkbox {
        if foot_text.contains("esc save/close") {
            prompt["cancel_keys"] = json!([]);
            actions.push(json!({"label":"Save and close","keys":["Escape"]}));
        }
        if foot_text.contains("enter save") && !is_memories {
            actions.push(json!({"label":"Save","keys":["Enter"]}));
        }
        if is_memories && choices[selected].checked.is_some() {
            actions.push(json!({"label":"Save memory settings","keys":["Enter"]}));
        }
        if foot_text.contains("←/→ reorder") {
            actions.push(json!({"label":"Move highlighted item earlier","keys":["Left"]}));
            actions.push(json!({"label":"Move highlighted item later","keys":["Right"]}));
        }
        actions.push(json!({"label":"Previous options","keys":["Up"]}));
        actions.push(json!({"label":"Next options","keys":["Down"]}));
    }
    if let Some(page) = page {
        prompt["page"] = json!({"current":page[2].parse::<usize>().ok()?,"total":page[3].parse::<usize>().ok()?});
        if &page[3] != "1" {
            actions.push(json!({"label":"Previous question","keys":["ctrl-p"]}));
            actions.push(json!({"label":"Next question","keys":["ctrl-n"]}));
        }
    }
    if is_question && !text_only {
        actions.push(json!({"label":"Add notes to highlighted choice","keys":["Tab"]}));
    }
    if !is_approval && !is_checkbox && !text_only && title != "Folder access" {
        actions.push(json!({"label":"Previous options","keys":["Up"]}));
        actions.push(json!({"label":"Next options","keys":["Down"]}));
        if !is_form {
            actions.push(json!({"label":"Previous page","keys":["PageUp"]}));
            actions.push(json!({"label":"Next page","keys":["PageDown"]}));
        }
    }
    if is_keymap {
        actions.push(json!({"label":"Previous shortcut group","keys":["Left"]}));
        actions.push(json!({"label":"Next shortcut group","keys":["Right"]}));
    }
    if !text_only
        && block[..foot]
            .iter()
            .any(|line| line.trim().starts_with("Type to search"))
    {
        prompt["text"] = json!({"label":"Search choices","before_keys":[],"after_keys":[],"multiline":false,"value":"","mode":"paste"});
    }
    if foot_text.contains(" · s session") {
        actions.push(json!({"label":"Apply highlighted choice for this session","keys":["s"]}));
    }
    if notes.is_some() {
        actions.push(json!({"label":"Clear notes and return to choices","keys":["Escape"]}));
    }
    if is_approval && foot_text.ends_with("or o to open thread") {
        actions.push(json!({"label":"Open thread","keys":["o"]}));
    }
    if text_only {
        let after = if title.starts_with("▌ Tell us more (") {
            "Enter".into()
        } else {
            key_hint(
                foot_text
                    .split(" to submit")
                    .next()?
                    .rsplit('|')
                    .next()?
                    .trim(),
            )?
        };
        let value = free
            .or(notes)
            .map(|index| {
                let first = block[index].trim().trim_start_matches('›').trim();
                let first = if first.starts_with("Type your answer") || first == "Add notes" {
                    ""
                } else {
                    first
                };
                std::iter::once(first)
                    .chain(block[index + 1..foot].iter().map(|line| line.trim()))
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        // No hidden clearing keystrokes: preserve any native draft for the PTY.
        if !value.is_empty() {
            return None;
        }
        prompt["text"] = json!({"label":if notes.is_some(){"Notes"}else{"Answer"},"before_keys":[],"after_keys":[after],"multiline":true,"value":value,"mode":"paste"});
    }
    if !actions.is_empty() {
        prompt["actions"] = json!(actions);
    }
    Some(prompt)
}
