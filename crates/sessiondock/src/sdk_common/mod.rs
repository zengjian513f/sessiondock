//! Shared checkpoint writes and diagnostics for the native SDK adapters.

pub(crate) mod projection;
pub(crate) mod usage;
pub(crate) mod writers;

use crate::lifecycle::launcher::CliProfile;
use crate::sessions::native_json;
use std::{io, path::Path};

pub(crate) async fn write_input(
    mut stdin: tokio::process::ChildStdin,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    failed: tokio_util::sync::CancellationToken,
) {
    use tokio::io::AsyncWriteExt;
    while let Some(frame) = rx.recv().await {
        let mut line = frame.to_string();
        line.push('\n');
        if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            failed.cancel();
            return;
        }
    }
    let _ = stdin.shutdown().await;
    failed.cancel();
}

#[cfg(target_os = "linux")]
pub(crate) fn process_owned(
    tree: &crate::runtime::procscan::ProcTree,
    pid: u32,
    own_pids: &std::collections::BTreeSet<u32>,
) -> bool {
    let mut current = pid;
    let mut visited = std::collections::BTreeSet::new();
    while current > 1 && visited.insert(current) {
        if own_pids.contains(&current) {
            return true;
        }
        let Some(parent) = tree.parent(current) else {
            break;
        };
        current = parent.ppid;
    }
    false
}

#[cfg(unix)]
pub(crate) fn checkpoint_file(root: &Path, uid: &str, sid: &str) -> std::path::PathBuf {
    let identity = if uid.is_empty() { sid } else { uid };
    let key: String = identity
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    root.join(format!("{key}.json"))
}

pub(crate) fn parse_json(text: &str) -> Result<serde_json::Value, native_json::ScanError> {
    serde_json::from_str(text).or_else(|_| {
        native_json::scan_value(
            text.as_bytes(),
            native_json::Limits {
                inline_string_bytes: text.len(),
            },
        )
    })
}

pub(crate) fn hosted_error_kind<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<&'static str>, D::Error> {
    let value = <Option<String> as serde::Deserialize>::deserialize(d)?;
    Ok(match value.as_deref() {
        Some("transport") => Some("transport"),
        Some("turn") => Some("turn"),
        Some("interrupted") => Some("interrupted"),
        _ => None,
    })
}

#[cfg(unix)]
pub(crate) fn private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

pub(crate) fn uuid_v4() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

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
