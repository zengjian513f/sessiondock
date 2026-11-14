//! OpenCode v2's documented `api` machine client. No terminal input, database
//! writes or private service-registration/credential-file reads.
use crate::lifecycle::launcher::CliProfile;
use base64::Engine;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

static REDACT_PATTERNS: std::sync::LazyLock<Vec<regex::Regex>> = std::sync::LazyLock::new(|| {
    [
        r#"(?i)(bearer\s+)[^\s"']+"#,
        r"(?i)(basic\s+)[A-Za-z0-9+/=]+",
        r"(https?://)[^/\s@]+:[^/\s@]+@",
    ]
    .into_iter()
    .map(|pattern| regex::Regex::new(pattern).expect("valid opencode redaction regex"))
    .collect()
});

pub(super) struct Transport {
    pub shutdown: CancellationToken,
    secrets: Vec<String>,
    commands: crate::sdk_common::commands::Commands,
}

impl Transport {
    pub fn new(
        profile: CliProfile,
        shutdown: CancellationToken,
        root: PathBuf,
        scope: &Path,
    ) -> std::io::Result<Self> {
        let mut secrets: Vec<String> = std::env::vars()
            .chain(profile.env.clone())
            .filter_map(|(name, value)| {
                let name = name.to_ascii_uppercase();
                (!value.is_empty()
                    && ["TOKEN", "SECRET", "PASSWORD", "API_KEY", "AUTH"]
                        .iter()
                        .any(|s| name.contains(s)))
                .then_some(value)
            })
            .collect();
        secrets.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        secrets.dedup();
        Ok(Self {
            commands: crate::sdk_common::commands::Commands::new(
                root,
                "opencode",
                scope,
                profile.clone(),
            )?,
            shutdown,
            secrets,
        })
    }

    pub async fn api(
        &self,
        cwd: &Path,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        // Require the shared service; do not intentionally start a daemon or
        // use a one-shot standalone runtime. The CLI's own discovery race is
        // documented in docs/opencode-sdk.md.
        let disabled = self
            .run(cwd, &["service".into(), "get".into(), "disabled".into()])
            .await?;
        if std::str::from_utf8(&disabled).unwrap_or_default().trim() != "false" {
            return Err(
                "OpenCode SDK 需要启用原生共享服务；不使用一次性 standalone runtime".into(),
            );
        }
        let status = self.run(cwd, &["service".into(), "status".into()]).await?;
        let status = std::str::from_utf8(&status).unwrap_or_default().trim();
        if !status.starts_with("http://") && !status.starts_with("https://") {
            return Err("OpenCode 原生共享服务未运行；请先启动 OpenCode 或 opencode service start (autostart=false)".into());
        }
        let mut args = vec!["api".into(), method.into(), path.into()];
        if let Some(body) = body {
            args.extend(["--data".into(), body.to_string()]);
        }
        let identity = body_identity(method, path, &args);
        let output = self.run_identified(cwd, &args, identity.as_deref()).await?;
        if output.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&output).map_err(|_| "OpenCode API 未返回 v2 JSON".into())
    }

    async fn run(&self, cwd: &Path, args: &[String]) -> Result<Vec<u8>, String> {
        self.run_identified(cwd, args, None).await
    }

    async fn run_identified(
        &self,
        cwd: &Path,
        args: &[String],
        identity: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let result = self
            .commands
            .run(cwd, args, identity, &self.shutdown)
            .await
            .map_err(|e| format!("OpenCode API: {e}"))?;
        let output: Vec<u8> = base64::engine::general_purpose::STANDARD
            .decode(result["stdout"].as_str().ok_or("OpenCode stdout missing")?)
            .map_err(|e| e.to_string())?;
        let diagnostic: Vec<u8> = base64::engine::general_purpose::STANDARD
            .decode(result["stderr"].as_str().ok_or("OpenCode stderr missing")?)
            .map_err(|e| e.to_string())?;
        if !diagnostic.iter().all(u8::is_ascii_whitespace) {
            crate::log::warn(
                "opencode_sdk.native_stderr",
                serde_json::json!({"detail":self.redact(&String::from_utf8_lossy(&diagnostic))}),
            );
        }
        if result["success"] != true {
            let native: Option<Value> = serde_json::from_slice(&output).ok();
            let detail = native
                .as_ref()
                .and_then(|v| v["message"].as_str())
                .unwrap_or_else(|| {
                    std::str::from_utf8(&diagnostic).unwrap_or("OpenCode API failed")
                });
            let mut detail = self.redact(detail);
            let diagnostic = self.redact(&String::from_utf8_lossy(&diagnostic));
            if !diagnostic.trim().is_empty() && !detail.contains(diagnostic.trim()) {
                detail.push_str("; ");
                detail.push_str(diagnostic.trim());
            }
            return Err(format!("OpenCode API {}: {}", result["code"], detail));
        }
        Ok(output)
    }

    fn redact(&self, text: &str) -> String {
        let mut result = text.to_owned();
        for secret in &self.secrets {
            result = result.replace(secret, "[redacted]");
        }
        // HTTP auth headers and URLs can also be supplied by a provider.
        for regex in &*REDACT_PATTERNS {
            result = regex.replace_all(&result, "${1}[redacted]").into_owned();
        }
        result
    }
}

pub(super) fn encoded(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

// Native creation/message identity is stable across facade retries. Read-only
// observations are fresh; repeated settings and interrupt remain explicit actions.
fn body_identity(method: &str, path: &str, args: &[String]) -> Option<String> {
    if method != "POST" {
        return None;
    }
    let body: Value = serde_json::from_str(args.last()?).ok()?;
    let id = body["id"].as_str()?;
    Some(format!("{method}:{path}:{id}"))
}
