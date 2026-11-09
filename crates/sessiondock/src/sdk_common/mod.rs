//! Shared checkpoint writes and diagnostics for the native SDK adapters.

pub(crate) mod projection;
pub(crate) mod usage;

use crate::lifecycle::launcher::CliProfile;
use std::{io, path::Path};

pub(crate) fn save_atomic(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension("tmp");
    let result = (|| -> io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(value).map_err(io::Error::other)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
static REDACT_PATTERNS: std::sync::LazyLock<Vec<regex::Regex>> = std::sync::LazyLock::new(|| {
    [
        r#"(?i)\bBearer\s+[^\s"']+"#,
        r"\bsk-[A-Za-z0-9_-]+",
        r"(?i)(?:api[_-]?key|token|password|secret)\s*[:=]\s*[^\s,;]+",
    ]
    .into_iter()
    .map(|pattern| regex::Regex::new(pattern).expect("valid SDK redaction regex"))
    .collect()
});

pub(crate) fn diagnostic_secrets(profile: &CliProfile) -> Vec<String> {
    let mut secrets: Vec<String> = std::env::vars()
        .chain(profile.env.clone())
        .filter_map(|(name, value)| {
            let name = name.to_ascii_uppercase();
            (value.len() >= 4
                && ["TOKEN", "KEY", "SECRET", "PASSWORD", "CREDENTIAL", "AUTH"]
                    .iter()
                    .any(|key| name.contains(key)))
            .then_some(value)
        })
        .collect();
    secrets.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    secrets.dedup();
    secrets
}

pub(crate) fn redact(mut text: String, secrets: &[String]) -> String {
    for secret in secrets {
        text = text.replace(secret, "[redacted]");
    }
    // Unknown secret-shaped values in SDK/HTTP diagnostics are redacted too.
    for regex in &*REDACT_PATTERNS {
        text = regex.replace_all(&text, "[redacted]").into_owned();
    }
    text
}
