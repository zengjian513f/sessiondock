//! CLI question cards and approvals shown live on the conversation page:
//! the `claude-hook` subcommand and its prompt
//! files, the strict Codex approval screen parser, and the `LivePrompts`
//! service that fills the `prompt` field of `/api/messages` and `/api/watch`.
//! Answering stays the page's own `/api/term/send` keyboard input under its
//! terminal lease; nothing here writes to a CLI.

pub mod claude;
pub mod codex;
pub mod live;

pub use claude::{HOOK_SUBCOMMAND, PROMPTS_DIRNAME, PromptStore};
pub use live::{CodexProbe, LivePrompts, PromptScope};
