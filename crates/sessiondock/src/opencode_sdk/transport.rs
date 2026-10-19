//! OpenCode v2's documented `api` machine client. No terminal input, database
//! writes or private service-registration/credential-file reads.
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde_json::Value;
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

use crate::lifecycle::launcher::{CliProfile, DENIED_ENV};

pub(super) struct Transport {
    pub profile: CliProfile,
    pub shutdown: CancellationToken,
}

// OpenCode's machine client can exit before a large pipe write drains. A
// regular private file makes its stdout synchronous; retain it through wait,
// read the complete response, and remove it on every return path.
struct CapturedOutput(PathBuf);

impl CapturedOutput {
    fn create() -> Result<(Self, std::fs::File), String> {
        let path = std::env::temp_dir().join(super::random_id("sessiondock-opencode-stdout-")?);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|e| format!("OpenCode API stdout 文件不可用: {e}"))?;
        Ok((Self(path), file))
    }
}

impl Drop for CapturedOutput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl Transport {
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
        let output = self.run(cwd, &args).await?;
        if output.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&output).map_err(|_| "OpenCode API 未返回 v2 JSON".into())
    }

    async fn run(&self, cwd: &Path, args: &[String]) -> Result<Vec<u8>, String> {
        if self.shutdown.is_cancelled() {
            return Err("OpenCode SDK 已关闭".into());
        }
        let executable = std::fs::canonicalize(&self.profile.executable)
            .map_err(|e| format!("OpenCode CLI 不可用: {e}"))?;
        let (capture, stdout) = CapturedOutput::create()?;
        let mut command = Command::new(executable);
        command.args(&self.profile.args).args(args);
        for name in &self.profile.env_remove {
            command.env_remove(name);
        }
        command.envs(&self.profile.env);
        for name in DENIED_ENV {
            command.env_remove(name);
        }
        command
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|e| format!("OpenCode API 启动失败: {e}"))?;
        let mut stderr = child.stderr.take().ok_or("OpenCode API stderr 不可用")?;
        let err = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let status = tokio::select! {
            result = child.wait() => result.map_err(|e| e.to_string()),
            _ = self.shutdown.cancelled() => Err("OpenCode SDK 已关闭".into()),
            _ = tokio::time::sleep(Duration::from_secs(30)) => Err("OpenCode API 请求超时；原生已接收的输入不会重发".into()),
        };
        if status.is_err() {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // Only the process group created above, never a TUI/server.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        let output = tokio::fs::read(&capture.0)
            .await
            .map_err(|e| e.to_string())?;
        let diagnostic = err
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if !diagnostic.iter().all(u8::is_ascii_whitespace) {
            crate::log::warn(
                "opencode_sdk.native_stderr",
                serde_json::json!({"detail":self.redact(&String::from_utf8_lossy(&diagnostic))}),
            );
        }
        let status = status?;
        if !status.success() {
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
            return Err(format!("OpenCode API {}: {}", status, detail));
        }
        Ok(output)
    }

    fn redact(&self, text: &str) -> String {
        let mut result = text.to_owned();
        for (name, value) in std::env::vars().chain(self.profile.env.clone()) {
            let name = name.to_ascii_uppercase();
            if !value.is_empty()
                && ["TOKEN", "SECRET", "PASSWORD", "API_KEY", "AUTH"]
                    .iter()
                    .any(|s| name.contains(s))
            {
                result = result.replace(&value, "[redacted]");
            }
        }
        // HTTP auth headers and URLs can also be supplied by a provider.
        for pattern in [
            r#"(?i)(bearer\s+)[^\s"']+"#,
            r"(?i)(basic\s+)[A-Za-z0-9+/=]+",
            r"(https?://)[^/\s@]+:[^/\s@]+@",
        ] {
            if let Ok(regex) = regex::Regex::new(pattern) {
                result = regex.replace_all(&result, "${1}[redacted]").into_owned();
            }
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
