//! Explicit Claude statusLine installation; pass the original stdin/stdout through.
use serde_json::{Value, json};
use std::{io::Read, path::Path, process::Stdio};
use tokio::io::AsyncWriteExt;

pub fn install(directory: &Path, settings: &Path) -> std::io::Result<()> {
    if cfg!(windows) {
        return Err(std::io::Error::other(
            "quota-install currently requires a POSIX shell",
        ));
    }
    let mut value: Value = match std::fs::read(settings) {
        Ok(b) => serde_json::from_slice(&b)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e),
    };
    if !value.is_object() {
        return Err(std::io::Error::other("settings must be an object"));
    }
    let executable = std::env::current_exe()?;
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let command = format!(
        "{} quota-statusline {}",
        quote(&executable.to_string_lossy()),
        quote(&directory.to_string_lossy())
    );
    if value["statusLine"]["command"] == command {
        return Ok(());
    }
    if value["statusLine"]["command"]
        .as_str()
        .is_some_and(|s| s.contains(" quota-statusline "))
    {
        return Err(std::io::Error::other(
            "another quota wrapper is already installed",
        ));
    }
    crate::quotas::save(
        &directory.join("quota-statusline.json"),
        &value["statusLine"],
    )?;
    if settings.exists() {
        let backup = settings.with_extension(format!(
            "quota-backup-{}.json",
            chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).timestamp_millis()
        ));
        std::fs::copy(settings, backup)?;
    }
    if !value["statusLine"].is_object() {
        value["statusLine"] = json!({});
    }
    value["statusLine"]["type"] = json!("command");
    value["statusLine"]["command"] = json!(command);
    crate::quotas::save(settings, &value)
}
pub async fn run(directory: &Path) {
    let mut bytes = Vec::new();
    if std::io::stdin().read_to_end(&mut bytes).is_err() {
        return;
    }
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && let Some(snapshot) = crate::quotas::claude_snapshot(&value)
    {
        let _ = crate::quotas::save(&directory.join("quotas/claude.json"), &snapshot);
    }
    let original = std::fs::read(directory.join("quota-statusline.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    if let Some(command) = original.as_ref().and_then(|v| v["command"].as_str()) {
        #[cfg(unix)]
        let mut process = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", command]);
            c
        };
        #[cfg(windows)]
        let mut process = {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", command]);
            c
        };
        if let Ok(mut child) = process
            .stdin(Stdio::piped())
            .stdout(Stdio::inherit())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                if let Some(mut input) = child.stdin.take() {
                    let _ = input.write_all(&bytes).await;
                }
                let _ = child.wait().await;
            })
            .await;
        }
    }
}
