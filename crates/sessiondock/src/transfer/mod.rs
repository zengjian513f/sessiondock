//! Whole-group transfer foundations. No publishing, process launch or deletion.
//! Plans enumerate physical history independently from sidebar visibility.
//! Clone output is private staging only until native metadata import is verified.

pub mod codex;
pub mod group;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferError {
    pub code: String,
    pub message: String,
}

impl TransferError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for TransferError {}

impl From<std::io::Error> for TransferError {
    fn from(error: std::io::Error) -> Self {
        Self::new("move_io", error.to_string())
    }
}

impl From<serde_json::Error> for TransferError {
    fn from(error: serde_json::Error) -> Self {
        Self::new("move_format", error.to_string())
    }
}
