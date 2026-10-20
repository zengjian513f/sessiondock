//! Verified Agy 1.4.2 NDJSON wire format (official headless CLI, not an app-server).
use crate::lifecycle::launcher::{CliProfile, DENIED_ENV};
use serde_json::{Value, json};
use std::{
    io,
    path::{Component, Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, Command},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

pub(super) struct ProcessConfig {
    profile: CliProfile,
    home: PathBuf,
    model_cwd: PathBuf,
    proc_root: PathBuf,
    secrets: Vec<String>,
    owned: std::sync::Mutex<std::collections::HashSet<u32>>,
    pub host_dir: Option<PathBuf>,
    pub adapter_dir: Option<PathBuf>,
}

pub(super) fn validate_sid(sid: &str) -> Result<(), String> {
    let mut parts = Path::new(sid).components();
    if matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none() {
        Ok(())
    } else {
        Err("agy_sdk_invalid_native_identity".into())
    }
}

impl ProcessConfig {
    pub fn new(
        profile: CliProfile,
        home: PathBuf,
        proc_root: PathBuf,
        state_dir: Option<&PathBuf>,
    ) -> io::Result<Self> {
        // The CLI has no documented --data-dir override. Never silently launch into another HOME.
        let launcher_home = profile.env.get("HOME").map(PathBuf::from).or_else(|| {
            (!profile.env_remove.iter().any(|key| key == "HOME"))
                .then(|| std::env::var_os("HOME").map(PathBuf::from))
                .flatten()
        });
        let expected = launcher_home
            .as_ref()
            .map(|home| home.join(".gemini/antigravity-cli"));
        let resolved = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if expected
            .as_deref()
            .is_none_or(|expected| resolved(expected) != resolved(&home))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agy_sdk_home_mismatch: launcher HOME must own configured AGY_HOME",
            ));
        }
        let sensitive = |key: &str| {
            let key = key.to_ascii_uppercase();
            ["KEY", "TOKEN", "SECRET", "PASSWORD", "AUTH", "CREDENTIAL"]
                .iter()
                .any(|part| key.contains(part))
        };
        let mut secrets: Vec<_> = std::env::vars()
            .chain(profile.env.clone())
            .filter(|(key, value)| sensitive(key) && !value.is_empty())
            .map(|(_, value)| value)
            .collect();
        secrets.sort_by_key(|value| std::cmp::Reverse(value.len()));
        secrets.dedup();
        Ok(Self {
            host_dir: state_dir.map(|dir| dir.join("sdkhost")),
            adapter_dir: state_dir.map(|dir| dir.join("sdk-adapters/agy")),
            profile,
            home,
            model_cwd: launcher_home.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "agy_sdk_home_missing")
            })?,
            proc_root,
            secrets,
            owned: std::sync::Mutex::new(std::collections::HashSet::new()),
        })
    }

    pub fn adopt(&self, pid: u32) {
        self.owned.lock().unwrap().insert(pid);
    }

    pub fn native_path(&self, sid: &str) -> PathBuf {
        self.home.join("conversations").join(format!("{sid}.db"))
    }

    fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(&self.profile.executable);
        command
            .args(&self.profile.args)
            .current_dir(cwd)
            .kill_on_drop(true);
        for key in &self.profile.env_remove {
            command.env_remove(key);
        }
        command.envs(&self.profile.env);
        for key in DENIED_ENV {
            command.env_remove(key);
        }
        command
    }

    pub fn redact(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in &self.secrets {
            text = text.replace(secret, "[redacted]");
        }
        // Retain useful diagnostics while removing recognizable credentials not present in profile.env.
        for pattern in [
            r"(?i)(bearer\s+)[^\s\x22']+",
            r"(?i)((?:api[_-]?key|access[_-]?token|refresh[_-]?token|password|secret|authorization)\s*[:=]\s*[\x22']?)[^\s,\x22'}]+",
            r"(https?://)[^/\s:@]+:[^/\s@]+@",
        ] {
            if let Ok(regex) = regex::Regex::new(pattern) {
                text = regex.replace_all(&text, "${1}[redacted]").into_owned();
            }
        }
        text
    }

    /// Agy has no documented shared local daemon or writer lease. Refuse observed native holders.
    /// Only inspect fd links, never other processes' environment or credentials.
    pub async fn check_available(self: &Arc<Self>, sid: String) -> Result<(), String> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.check_holders(&sid))
            .await
            .map_err(|_| "agy_sdk_process_scan_failed".to_owned())?
    }

    fn check_holders(&self, sid: &str) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            let native = self.native_path(sid);
            let canonical = native.canonicalize().unwrap_or_else(|_| native.clone());
            let identity = std::fs::metadata(&native)
                .ok()
                .map(|meta| (meta.dev(), meta.ino()));
            let entries = std::fs::read_dir(&self.proc_root)
                .map_err(|error| format!("agy_sdk_process_scan: {error}"))?;
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                    .is_none()
                {
                    continue;
                }
                if entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                    .is_some_and(|pid| self.owned.lock().unwrap().contains(&pid))
                {
                    continue;
                }
                let directory = entry.path();
                // SAFETY: geteuid has no arguments or memory effects.
                if directory
                    .metadata()
                    .ok()
                    .is_some_and(|meta| meta.uid() != unsafe { libc::geteuid() })
                {
                    continue;
                }
                let Ok(fds) = std::fs::read_dir(directory.join("fd")) else {
                    continue;
                };
                for fd in fds.flatten() {
                    let link = fd.path();
                    let target = std::fs::read_link(&link).ok();
                    let same_path = target
                        .as_ref()
                        .is_some_and(|path| path == &native || path == &canonical);
                    let same_inode = identity.is_some_and(|identity| {
                        std::fs::metadata(&link)
                            .is_ok_and(|meta| (meta.dev(), meta.ino()) == identity)
                    });
                    if same_path || same_inode {
                        return Err("agy_sdk_session_busy: 原生 conversation 数据库正被独立 Agy/TUI 进程使用；SDK 不接管或终止该进程，请在原生界面正常退出后重试。".into());
                    }
                }
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = sid;
            Err("agy_sdk_coexistence_unsupported: 此平台没有已验证的 Agy 原生占用检查。".into())
        }
    }

    pub async fn models(&self, shutdown: CancellationToken) -> Result<Value, String> {
        let mut command = self.command(&self.model_cwd);
        command
            .arg("models")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = tokio::select! {
            _=shutdown.cancelled()=>return Err("agy_sdk_shutdown".into()),
            output=tokio::time::timeout(Duration::from_secs(15),command.output())=>output.map_err(|_|"agy_sdk_models_timeout")?.map_err(|error|format!("agy_sdk_models: {error}"))?,
        };
        if !output.status.success() {
            return Err(format!(
                "agy_sdk_models_failed: {}; {}",
                output.status,
                self.redact(&String::from_utf8_lossy(&output.stderr))
            ));
        }
        let models:Vec<_>=String::from_utf8_lossy(&output.stdout).lines().filter_map(|line| {
            let (id,label)=line.split_once('\t')?;
            if id.is_empty(){return None;}
            Some(json!({"id":id,"model":id,"name":label.trim(),"displayName":label.trim(),"efforts":["low","medium","high","xhigh","max"]}))
        }).collect();
        Ok(json!({"models":models,"data":models,"capabilities":super::capabilities()}))
    }
}

pub(super) struct Native {
    child: Option<Child>,
    host: Option<sdkhost_client::Client>,
    pump: Option<JoinHandle<()>>,
    pub input: Option<ChildStdin>,
    pub output: Lines<BufReader<std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>>>,
    stderr: Option<JoinHandle<String>>,
    pub sid: String,
    pub cwd: String,
    process: Arc<ProcessConfig>,
    pid: u32,
}

impl Drop for Native {
    fn drop(&mut self) {
        self.process.owned.lock().unwrap().remove(&self.pid);
        if let Some(pump) = self.pump.take() {
            pump.abort();
        }
        if let Some(stderr) = self.stderr.take() {
            stderr.abort();
        }
    }
}

impl Native {
    pub async fn start(
        process: Arc<ProcessConfig>,
        sid: Option<&str>,
        cwd: &Path,
        model: Option<&str>,
        effort: Option<&str>,
        mode: Option<&str>,
        shutdown: CancellationToken,
        binding: Option<(&str, bool)>,
    ) -> Result<Self, String> {
        if shutdown.is_cancelled() {
            return Err("agy_sdk_shutdown".into());
        }
        let mut command = process.command(cwd);
        command.args([
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
        ]);
        if let Some(sid) = sid {
            command.arg("--conversation").arg(sid);
        }
        if let Some(model) = model {
            command.arg("--model").arg(model);
        }
        if let Some(effort) = effort {
            command.arg("--effort").arg(effort);
        }
        if let Some(mode) = mode {
            command.arg("--mode").arg(mode);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let (child, host, input, stdout, stderr, pump, pid): (
            Option<Child>,
            Option<sdkhost_client::Client>,
            Option<ChildStdin>,
            std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>,
            Option<JoinHandle<String>>,
            Option<JoinHandle<()>>,
            u32,
        ) = if let (Some(dir), Some((key, recovering))) = (&process.host_dir, binding) {
            let host = if recovering {
                sdkhost_client::Client::attach(dir, key).await
            } else {
                sdkhost_client::Client::launch_or_attach(dir, key, &command).await
            }
            .map_err(|error| format!("agy_sdk_host: {error}"))?;
            let pid = host
                .status()
                .await
                .map_err(|e| format!("agy_sdk_host: {e}"))?
                .pid;
            let (reader, mut writer) = tokio::io::duplex(65536);
            let stream_host = host.clone();
            let pump = tokio::spawn(async move {
                let mut offset = 0;
                loop {
                    match stream_host
                        .read(sdkhost_client::Stream::Stdout, offset)
                        .await
                    {
                        Ok(chunk) => {
                            offset = chunk.next_offset;
                            if writer.write_all(&chunk.bytes).await.is_err() || chunk.eof {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
            (
                None,
                Some(host),
                None,
                Box::pin(reader),
                None,
                Some(pump),
                pid,
            )
        } else {
            let mut child = command
                .spawn()
                .map_err(|error| format!("agy_sdk_spawn: {error}"))?;
            let pid = child.id().ok_or("agy_sdk_child_missing")?;
            let input = child.stdin.take();
            let stdout = child.stdout.take().ok_or("agy_sdk_stdout_missing")?;
            let mut stderr = child.stderr.take().ok_or("agy_sdk_stderr_missing")?;
            let redactor = process.clone();
            let stderr = tokio::spawn(async move {
                let mut bytes = Vec::new();
                let _ = stderr.read_to_end(&mut bytes).await;
                redactor.redact(&String::from_utf8_lossy(&bytes))
            });
            (
                Some(child),
                None,
                input,
                Box::pin(stdout),
                Some(stderr),
                None,
                pid,
            )
        };
        process.owned.lock().unwrap().insert(pid);
        let mut native = Self {
            child,
            host,
            input,
            output: BufReader::new(stdout).lines(),
            stderr,
            pump,
            sid: String::new(),
            cwd: String::new(),
            process: process.clone(),
            pid,
        };
        let init = tokio::select! {
            _=shutdown.cancelled()=>Err("agy_sdk_shutdown".to_owned()),
            line=tokio::time::timeout(Duration::from_secs(20),native.output.next_line())=>match line {
                Ok(Ok(Some(line)))=>serde_json::from_str::<Value>(&line).map_err(|_|"agy_sdk_invalid_init_json".to_owned()),
                Ok(Ok(None))=>Err("agy_sdk_no_init".into()),
                Ok(Err(error))=>Err(format!("agy_sdk_init_read: {error}")),
                Err(_)=>Err("agy_sdk_init_timeout".into()),
            },
        };
        let result = (|| {
            let init = init?;
            if init["event"] != "init" {
                return Err(format!(
                    "agy_sdk_init_failed: {}",
                    process.redact(
                        init["result"]["error"]
                            .as_str()
                            .unwrap_or("expected init event")
                    )
                ));
            }
            let actual_sid = init["conversation_id"]
                .as_str()
                .ok_or("agy_sdk_init_missing_sid")?;
            validate_sid(actual_sid)?;
            if sid.is_some_and(|sid| sid != actual_sid) {
                return Err("agy_sdk_native_sid_mismatch".into());
            }
            let actual_cwd = init["init"]["cwd"]
                .as_str()
                .ok_or("agy_sdk_init_missing_cwd")?;
            let expected_cwd = cwd.canonicalize().map_err(|_| "agy_sdk_cwd_unavailable")?;
            let actual_directory = Path::new(actual_cwd)
                .canonicalize()
                .map_err(|_| "agy_sdk_native_cwd_unavailable")?;
            if actual_directory != expected_cwd {
                return Err("agy_sdk_native_cwd_mismatch".into());
            }
            if let Some(model) = model
                && init["init"]["model"].as_str() != Some(model)
            {
                return Err("agy_sdk_native_model_mismatch".into());
            }
            native.sid = actual_sid.to_owned();
            native.cwd = actual_cwd.to_owned();
            Ok(())
        })();
        if let Err(error) = result {
            if !shutdown.is_cancelled() {
                native.terminate().await;
            }
            let diagnostic = native.diagnostics().await;
            return Err(if diagnostic.is_empty() {
                error
            } else {
                format!("{error}; {diagnostic}")
            });
        }
        Ok(native)
    }

    pub async fn send(&mut self, text: &str) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&json!({"event":"user","message":{"content":text}}))
            .map_err(|_| "agy_sdk_encode")?;
        bytes.push(b'\n');
        if let Some(host) = &self.host {
            let receipt = match host
                .receipt("input")
                .await
                .map_err(|e| format!("agy_sdk_write_uncertain: {e}"))?
            {
                Some(receipt) => receipt,
                None => host
                    .write("input", &bytes)
                    .await
                    .map_err(|e| format!("agy_sdk_write_uncertain: {e}"))?,
            };
            if receipt.state != "written" {
                return Err(format!("agy_sdk_write_uncertain: {}", receipt.state));
            }
            return Ok(());
        }
        let input = self.input.as_mut().ok_or("agy_sdk_stdin_closed")?;
        input
            .write_all(&bytes)
            .await
            .map_err(|error| format!("agy_sdk_write_uncertain: {error}; 输入不会自动重发。"))?;
        input
            .flush()
            .await
            .map_err(|error| format!("agy_sdk_flush_uncertain: {error}; 输入不会自动重发。"))
    }

    pub fn check_event(&self, event: &Value) -> Result<(), String> {
        let sid = event["conversation_id"]
            .as_str()
            .or_else(|| event["step_update"]["conversation_id"].as_str())
            .or_else(|| event["result"]["conversation_id"].as_str());
        if sid.is_some_and(|sid| sid != self.sid) {
            Err("agy_sdk_native_sid_mismatch".into())
        } else {
            Ok(())
        }
    }

    pub async fn close_input(&mut self) -> Result<(), String> {
        if let Some(host) = &self.host {
            if !host
                .status()
                .await
                .map_err(|e| format!("agy_sdk_eof: {e}"))?
                .stdin_closed
            {
                host.close_stdin("eof")
                    .await
                    .map_err(|e| format!("agy_sdk_eof: {e}"))?;
            }
        } else {
            self.input.take();
        }
        Ok(())
    }

    pub async fn interrupt(&mut self) -> Result<(), String> {
        if let Some(host) = &self.host {
            return host
                .signal(libc::SIGINT)
                .await
                .map_err(|e| format!("agy_sdk_interrupt: {e}"));
        }
        #[cfg(unix)]
        {
            let pid = self
                .child
                .as_ref()
                .and_then(|child| child.id())
                .ok_or("agy_sdk_idle")?;
            // SAFETY: this retained Child owns the PID until wait/reap; never signal discovered processes.
            if unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) } != 0 {
                return Err(format!("agy_sdk_interrupt: {}", io::Error::last_os_error()));
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err("agy_sdk_interrupt_unsupported".into())
        }
    }

    pub async fn terminate(&mut self) {
        if let Some(host) = &self.host {
            #[cfg(unix)]
            let _ = host.signal(libc::SIGKILL).await;
            let _ = host.wait().await;
        } else if let Some(child) = &mut self.child {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    pub async fn finish(&mut self, shutdown: CancellationToken) -> Result<ExitStatus, String> {
        #[cfg(unix)]
        if let Some(host) = &self.host {
            use std::os::unix::process::ExitStatusExt;
            return tokio::select! {
                _=shutdown.cancelled()=>Err("agy_sdk_shutdown".into()),
                result=host.wait()=>result.map(|exit| ExitStatus::from_raw(
                    exit.code.map_or_else(|| exit.signal.unwrap_or(1), |code| code << 8)
                )).map_err(|error|format!("agy_sdk_wait: {error}")),
            };
        }
        let child = self.child.as_mut().ok_or("agy_sdk_child_missing")?;
        tokio::select! {
            _=shutdown.cancelled()=>Err("agy_sdk_shutdown".into()),
            result=child.wait()=>result.map_err(|error|format!("agy_sdk_wait: {error}")),
        }
    }

    pub async fn diagnostics(&mut self) -> String {
        if let Some(host) = &self.host {
            let mut bytes = Vec::new();
            let mut offset = 0;
            while let Ok(chunk) = host.read(sdkhost_client::Stream::Stderr, offset).await {
                let empty = chunk.bytes.is_empty();
                offset = chunk.next_offset;
                bytes.extend(chunk.bytes);
                if chunk.eof || empty {
                    break;
                }
            }
            return self.process.redact(&String::from_utf8_lossy(&bytes));
        }
        let Some(mut task) = self.stderr.take() else {
            return String::new();
        };
        match tokio::time::timeout(Duration::from_secs(1), &mut task).await {
            Ok(Ok(text)) => text,
            _ => {
                task.abort();
                "agy_sdk_stderr: diagnostic pipe did not close\n".into()
            }
        }
    }

    pub async fn close(mut self, shutdown: CancellationToken) -> Result<String, String> {
        self.input.take();
        // Startup-only process has received no user message; EOF asks the native CLI to exit.
        let result = tokio::time::timeout(Duration::from_secs(5), self.finish(shutdown)).await;
        match result {
            Ok(Ok(status)) if status.success() => Ok(self.diagnostics().await),
            Ok(Ok(status)) => Err(format!(
                "agy_sdk_create_exit: {status}; {}",
                self.diagnostics().await
            )),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                self.terminate().await;
                Err("agy_sdk_create_close_timeout".into())
            }
        }
    }
}
