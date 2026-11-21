//! Native usage accounting runs off the request path and never starts an agent.
use crate::{config::Config, quotas::Quotas};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const SCRIPT: &str = include_str!("../../../tools/agent_usage.py");
const PRICE_SYNC: &str = include_str!("../../../tools/agent_usage_prices.py");
const PRICES: &str = include_str!("../../../tools/agent_usage_prices.json");

pub struct AgentUsage {
    directory: Option<PathBuf>,
    config: Value,
    quotas: Arc<Quotas>,
    running: AtomicBool,
    edit: Mutex<()>,
    shutdown: CancellationToken,
    error: Mutex<Option<String>>,
}

impl AgentUsage {
    pub fn new(config: &Config, quotas: Arc<Quotas>, shutdown: CancellationToken) -> Arc<Self> {
        let roots = [
            ("codex", &config.roots.codex),
            ("claude", &config.roots.claude),
            ("grok", &config.roots.grok),
            ("agy", &config.agy_home),
            ("kiro", &config.kiro_native_root),
            ("droid", &config.roots.droid),
            ("codebuddy", &config.roots.codebuddy),
            ("opencode", &config.opencode_db),
        ]
        .into_iter()
        .filter_map(|(name, path)| path.as_ref().map(|p| (name.to_string(), json!(p))))
        .collect::<serde_json::Map<String, Value>>();
        let directory = config.state_dir.as_ref().map(|p| p.join("agent-usage"));
        let value = directory.as_ref().map(|dir|json!({"directory":dir,"roots":roots,
            "quota_dir":config.state_dir.as_ref().unwrap().join("quotas"),"prices":dir.join("prices.json"),
            "default_prices":dir.join("default-prices.json"),
            "price_sync":{"enabled":std::env::var("SESSIONDOCK_USAGE_PRICE_SYNC").as_deref()!=Ok("off"),
                "url":std::env::var("SESSIONDOCK_USAGE_PRICE_URL").unwrap_or_else(|_|"https://developers.openai.com/api/docs/pricing.md".into())},
            "transfer_dir":config.state_dir.as_ref().unwrap().join("transfers"),
            "trash_dir":config.trash_dir,
            "codex_archive":config.roots.codex.as_ref().and_then(|p|p.parent()).map(|p|p.join("archived_sessions"))})).unwrap_or(Value::Null);
        let service = Arc::new(Self {
            directory,
            config: value,
            quotas,
            running: AtomicBool::new(false),
            edit: Mutex::new(()),
            shutdown,
            error: Mutex::new(None),
        });
        let task = service.clone();
        tokio::spawn(async move {
            // Let startup and identity recovery finish before scanning any native data.
            let mut due = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                let settings = task.settings().await;
                let seconds = settings["interval_seconds"].as_u64().unwrap_or(300).max(1);
                tokio::select! { _=task.shutdown.cancelled()=>break, _=tokio::time::sleep(Duration::from_secs(seconds.min(30)))=>{} }
                if task.settings().await["enabled"] == false {
                    continue;
                }
                if std::time::Instant::now() >= due {
                    task.start(true);
                    due = std::time::Instant::now() + Duration::from_secs(seconds);
                }
            }
        });
        service
    }

    async fn settings(&self) -> Value {
        let Some(directory) = &self.directory else {
            return json!({});
        };
        tokio::fs::read(directory.join("settings.json"))
            .await
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(json!({"enabled":true,"interval_seconds":300,"sources":{}}))
    }

    pub async fn read(&self) -> Value {
        let mut value = if let Some(dir) = &self.directory {
            tokio::fs::read(dir.join("snapshot.json"))
                .await
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .unwrap_or(json!({"rows":[]}))
        } else {
            json!({"rows":[]})
        };
        value["configured"] = json!(self.directory.is_some());
        value["running"] = json!(self.running.load(Ordering::Acquire));
        value["settings"] = self.settings().await;
        value["error"] = json!(*self.error.lock().await);
        value
    }

    pub fn start(self: &Arc<Self>, collect: bool) {
        if self.directory.is_none()
            || self.shutdown.is_cancelled()
            || self.running.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let task = self.clone();
        tokio::spawn(async move {
            let result = task.run(collect).await;
            *task.error.lock().await = result
                .err()
                .map(|_| "采集失败，保留旧值；请检查 Python 3 与统计目录".into());
            task.running.store(false, Ordering::Release);
        });
    }

    async fn run(&self, collect: bool) -> std::io::Result<()> {
        let _guard = self.edit.lock().await;
        let Some(dir) = &self.directory else {
            return Ok(());
        };
        let dir = dir.clone();
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            }
            crate::quotas::save(&dir.join("config.json"), &config)?;
            // Code and default prices belong to the running committed artifact.
            std::fs::write(dir.join("collector.py"), SCRIPT)?;
            std::fs::write(dir.join("agent_usage_prices.py"), PRICE_SYNC)?;
            if !dir.join("price-base.json").exists() {
                let previous = std::fs::read(dir.join("default-prices.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .unwrap_or(serde_json::from_str::<Value>(PRICES)?);
                crate::quotas::save(&dir.join("price-base.json"), &previous)?;
            }
            crate::quotas::save(
                &dir.join("default-prices.json"),
                &serde_json::from_str::<Value>(PRICES)?,
            )?;
            if !dir.join("prices.json").exists() {
                crate::quotas::save(
                    &dir.join("prices.json"),
                    &serde_json::from_str::<Value>(PRICES)?,
                )?;
            }
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)??;
        if collect {
            tokio::select! { _=self.shutdown.cancelled()=>return Ok(()), _=self.quotas.collect_cached(None)=>{} }
        }
        let python = std::env::var_os("SESSIONDOCK_USAGE_PYTHON")
            .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.into());
        let mut command = tokio::process::Command::new(python);
        command
            .arg(self.directory.as_ref().unwrap().join("collector.py"))
            .arg(if collect { "collect" } else { "report" })
            .arg("--config")
            .arg(self.directory.as_ref().unwrap().join("config.json"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn()?;
        let status = tokio::select! { _=self.shutdown.cancelled()=>{child.kill().await?;return Ok(())}, result=child.wait()=>result? };
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("usage collector failed"))
        }
    }

    pub async fn configure(self: &Arc<Self>, value: Value) -> Result<(), &'static str> {
        let Some(dir) = &self.directory else {
            return Err("未配置统计目录");
        };
        let _guard = self.edit.lock().await;
        // Only presentation/schedule fields are writable from a browser; native
        // read roots and executable paths stay in server configuration.
        let enabled = value["enabled"].as_bool().ok_or("需要启用状态")?;
        let interval = value["interval_seconds"]
            .as_u64()
            .filter(|v| *v > 0)
            .ok_or("更新间隔必须大于零")?;
        let mut sources = serde_json::Map::new();
        if let Some(entries) = value["sources"].as_object() {
            for (source, entry) in entries {
                if self.config["roots"].get(source).is_none() {
                    continue;
                }
                let mut windows = serde_json::Map::new();
                if let Some(entries) = entry["windows"].as_object() {
                    for (key, window) in entries {
                        if let Some(start) = window["start"].as_str() {
                            chrono::DateTime::parse_from_rfc3339(start)
                                .map_err(|_| "周期起点需要带时区的时间")?;
                            let reset = window["reset"].as_str().ok_or("需要当前窗口重置时间")?;
                            chrono::DateTime::parse_from_rfc3339(reset)
                                .map_err(|_| "重置时间需要带时区")?;
                            windows.insert(key.clone(), json!({"start":start,"reset":reset}));
                        }
                    }
                }
                sources.insert(
                    source.clone(),
                    json!({"account_complete":entry["account_complete"]==true,"windows":windows}),
                );
            }
        }
        let path = dir.join("settings.json");
        tokio::task::spawn_blocking(move || {
            crate::quotas::save(
                &path,
                &json!({"enabled":enabled,"interval_seconds":interval,"sources":sources}),
            )
        })
        .await
        .map_err(|_| "设置未保存")?
        .map_err(|_| "设置未保存")?;
        Ok(())
    }
}
