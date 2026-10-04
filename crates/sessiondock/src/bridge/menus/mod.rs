//! Current-screen projection of native CLI menus. Parsing describes actions;
//! the browser still sends explicit user choices through terminal ownership.
pub mod agy;
pub mod claude;
pub mod codex;
pub mod grok;
pub mod opencode;

pub fn screen_prompt(source: &str, screen: &str) -> Option<serde_json::Value> {
    match source {
        "claude" => super::claude::startup_prompt(screen).or_else(|| claude::screen_prompt(screen)),
        "codex" => super::codex::startup_prompt(screen).or_else(|| codex::screen_prompt(screen)),
        "grok" => grok::screen_prompt(screen),
        "opencode" => opencode::screen_prompt(screen),
        "agy" => agy::screen_prompt(screen),
        _ => None,
    }
}
