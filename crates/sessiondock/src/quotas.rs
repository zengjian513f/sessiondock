//! Account quota snapshots. No conversation is opened and no inference is requested.
use crate::lifecycle::{
    launcher::{CliProfile, Config},
    model::Source,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::Mutex,
};

pub struct Quotas {
    directory: Option<PathBuf>,
    profiles: Vec<CliProfile>,
    /// Kiro account HOME derived from the configured scope root, as for SDK sessions.
    kiro_home: Option<PathBuf>,
    refresh: [Mutex<Option<Instant>>; 3],
}
const COLLECTED: [&str; 3] = ["codex", "agy", "kiro"];
fn now() -> String {
    chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now()).to_rfc3339()
}
fn empty(source: &str) -> Value {
    json!({"source":source,"captured_at":null,"buckets":[],"error":null})
}
fn bucket(id: &str, group: &str, window: Value, remaining: Option<f64>, reset: Value) -> Value {
    json!({"id":id,"group":group,"window":window,"remaining_percent":remaining,"resets_at":reset})
}
fn epoch(value: &Value) -> Value {
    value
        .as_i64()
        .and_then(|v| chrono::DateTime::from_timestamp(v, 0))
        .map(|v| json!(v.to_rfc3339()))
        .unwrap_or(Value::Null)
}
fn snapshot(source: &str, buckets: Vec<Value>) -> Value {
    json!({"source":source,"captured_at":now(),"buckets":buckets,"error":null})
}
pub fn save(path: &Path, value: &Value) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let temp = path.with_extension(format!(
        "{}.tmp",
        getrandom::u64().map_err(std::io::Error::other)?
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
impl Quotas {
    pub fn new(state: Option<&Path>, config: Option<&Config>, kiro_root: Option<&Path>) -> Self {
        Self {
            directory: state.map(|p| p.join("quotas")),
            profiles: config.map(|c| c.profiles.clone()).unwrap_or_default(),
            kiro_home: kiro_root
                .and_then(crate::sessions::kiro::account_home)
                .map(Path::to_path_buf),
            refresh: std::array::from_fn(|_| Mutex::new(None)),
        }
    }
    pub async fn read(&self) -> Value {
        let claude_configured = match &self.directory {
            Some(directory) => {
                tokio::fs::try_exists(directory.parent().unwrap().join("quota-statusline.json"))
                    .await
                    .unwrap_or(false)
            }
            None => false,
        };
        let mut agents = Vec::new();
        for source in ["codex", "claude", "agy", "kiro"] {
            let mut value = empty(source);
            if let Some(dir) = &self.directory
                && let Ok(bytes) = tokio::fs::read(dir.join(format!("{source}.json"))).await
                && let Ok(saved) = serde_json::from_slice::<Value>(&bytes)
            {
                value = saved;
            }
            value["configured"] = json!(
                self.directory.is_some()
                    && ((source == "claude" && claude_configured)
                        || (source != "claude"
                            && self
                                .profiles
                                .iter()
                                .any(|p| source_name(p.source) == source)))
            );
            agents.push(value);
        }
        json!({"agents":agents})
    }
    pub async fn collect(&self) -> Value {
        self.collect_selected(None, false).await
    }
    pub async fn collect_source(&self, source: &str) -> Value {
        self.collect_selected(Some(source), false).await
    }
    pub async fn collect_cached(&self, source: Option<&str>) -> Value {
        self.collect_selected(source, true).await
    }
    async fn collect_selected(&self, selected: Option<&str>, cached: bool) -> Value {
        let requested_at = Instant::now();
        let Some(dir) = &self.directory else {
            return self.read().await;
        };
        let collect = async |source: &str| {
            if selected.is_some_and(|selected| selected != source) {
                return None;
            }
            let profile = self
                .profiles
                .iter()
                .find(|p| source_name(p.source) == source)?;
            let slot = COLLECTED.iter().position(|name| *name == source)?;
            let mut completed = self.refresh[slot].lock().await;
            let path = dir.join(format!("{source}.json"));
            let previous = tokio::fs::read(&path)
                .await
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .unwrap_or_else(|| empty(source));
            // Share in-flight queries across settings and every SDK session.
            // Automatic reads also reuse recent persisted successes or failures.
            let recent = ["captured_at", "attempted_at"].into_iter().any(|key| {
                previous[key]
                    .as_str()
                    .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                    .map(|time| {
                        chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
                            .signed_duration_since(time)
                            .num_seconds()
                    })
                    .is_some_and(|age| (0..60).contains(&age))
            });
            if completed.is_some_and(|time| time >= requested_at)
                || (cached
                    && (recent
                        || completed.is_some_and(|time| time.elapsed() < Duration::from_secs(60))))
            {
                return None;
            }
            let result = tokio::time::timeout(
                Duration::from_secs(25),
                query(profile, self.kiro_home.as_deref()),
            )
            .await;
            let mut value = match result {
                Ok(Ok(value)) => value,
                _ => {
                    let mut previous = previous;
                    previous["error"] = json!(if result.is_err() {
                        "查询超时，保留上次快照"
                    } else {
                        "查询失败，请检查 CLI 登录及版本"
                    });
                    previous["attempted_at"] = json!(now());
                    previous
                }
            };
            if save(&path, &value).is_err() {
                value["error"] = json!("快照保存失败，本次结果仅在当前页面有效");
            }
            *completed = Some(Instant::now());
            value["configured"] = json!(true);
            Some(value)
        };
        let (codex, agy, kiro) = tokio::join!(collect("codex"), collect("agy"), collect("kiro"));
        let mut data = self.read().await;
        for value in [codex, agy, kiro].into_iter().flatten() {
            if let Some(row) = data["agents"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|row| row["source"] == value["source"])
            {
                *row = value;
            }
        }
        data
    }
}
fn source_name(source: Source) -> &'static str {
    match source {
        Source::Codex => "codex",
        Source::Claude => "claude",
        Source::Agy => "agy",
        Source::Kiro => "kiro",
        _ => "",
    }
}
async fn query(profile: &CliProfile, kiro_home: Option<&Path>) -> Result<Value, ()> {
    let mut command = Command::new(&profile.executable);
    crate::lifecycle::launcher::background_command(command.as_std_mut());
    command
        .args(&profile.args)
        .envs(&profile.env)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in profile
        .env_remove
        .iter()
        .map(String::as_str)
        .chain(crate::lifecycle::launcher::DENIED_ENV)
    {
        command.env_remove(key);
    }
    if profile.source == Source::Kiro {
        return kiro_query(command, profile, kiro_home).await;
    }
    if profile.source == Source::Agy {
        if profile
            .args
            .iter()
            .any(|arg| arg.starts_with("--disable-slash-commands"))
        {
            return Err(());
        }
        let output = command
            .args(["-p", "/usage", "--output-format", "json"])
            .output()
            .await
            .map_err(|_| ())?;
        if !output.status.success() {
            return Err(());
        }
        let value: Value = serde_json::from_slice(&output.stdout).map_err(|_| ())?;
        if value["num_turns"] != 0
            || value["status"] != "SUCCESS"
            || value["command"]["name"] != "usage"
        {
            return Err(());
        }
        let groups = value["command"]["data"]["groups"].as_array().ok_or(())?;
        let mut buckets = Vec::new();
        for group in groups {
            for b in group["buckets"].as_array().ok_or(())? {
                buckets.push(bucket(
                    b["id"].as_str().unwrap_or(""),
                    group["name"].as_str().unwrap_or(""),
                    b["window"].clone(),
                    b["remaining_fraction"].as_f64().map(|v| v * 100.),
                    b["reset_time"].clone(),
                ));
            }
        }
        return Ok(snapshot("agy", buckets));
    }
    let mut child = command
        .args(["app-server", "--stdio"])
        .spawn()
        .map_err(|_| ())?;
    let mut input = child.stdin.take().ok_or(())?;
    let mut lines = BufReader::new(child.stdout.take().ok_or(())?).lines();
    input.write_all(b"{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"sessiondock-quota\",\"version\":\"1\"}}}\n").await.map_err(|_|())?;
    let result=async {
        loop {
            let line=lines.next_line().await.map_err(|_|())?.ok_or(())?;
            let value:Value=serde_json::from_str(&line).map_err(|_|())?;
            if value["id"]==1 {
                if value.get("error").is_some() {return Err(())}
                input.write_all(b"{\"method\":\"initialized\"}\n{\"id\":2,\"method\":\"account/rateLimits/read\"}\n").await.map_err(|_|())?;
            }
            if value["id"]==2 {
                if value.get("error").is_some() {return Err(())}
                let result=&value["result"];
                let limits:Vec<(&str,&Value)>=match result["rateLimitsByLimitId"].as_object() {
                    Some(map) if !map.is_empty()=>map.iter().map(|(k,v)|(k.as_str(),v)).collect(),
                    _=>vec![("codex",&result["rateLimits"])],
                };
                let mut buckets=Vec::new();
                let mut credit_rows=Vec::new();
                for (id,limit) in limits {
                    if limit["credits"].is_object() {
                        let c=&limit["credits"];
                        let remaining=if c["unlimited"]==true {json!("不限量")} else {c["balance"].clone()};
                        credit_rows.push(json!({"id":format!("{id}:credits"),"group":format!("{} credits",limit["limitName"].as_str().unwrap_or(id)),"remaining_text":remaining}));
                    }
                    for window in ["primary","secondary"] {
                        let b=&limit[window]; if !b.is_object() {continue}
                        buckets.push(bucket(&format!("{id}:{window}"),limit["limitName"].as_str().unwrap_or(id),b["windowDurationMins"].clone(),b["usedPercent"].as_f64().map(|v|100.-v),epoch(&b["resetsAt"])));
                    }
                }
                let resets=&result["rateLimitResetCredits"];
                if buckets.is_empty() && credit_rows.is_empty() && !resets.is_object() {return Err(())}
                if credit_rows.is_empty() {credit_rows.push(json!({"id":"credits","group":"Credits","remaining_text":null}));}
                buckets.extend(credit_rows);
                // The authoritative count can exceed the number of detail rows.
                let expiry=resets["credits"].as_array().and_then(|rows|rows.iter()
                    .filter(|c|c["status"]=="available").filter_map(|c|c["expiresAt"].as_i64()).min());
                buckets.push(json!({"id":"reset-credits","group":"重置卡","remaining_text":resets["availableCount"].as_i64().map(|n|format!("{n} 张")),"expires_at":epoch(&json!(expiry))}));
                return Ok(snapshot("codex",buckets));
            }
        }
    }.await;
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}
/// Kiro account usage through the CLI's own ACP `_kiro/account/getUsage` handler.
/// It is not listed in `extensionMethods`; an unsupported CLI fails like any query.
async fn kiro_query(
    mut command: Command,
    profile: &CliProfile,
    home: Option<&Path>,
) -> Result<Value, ()> {
    if !profile.env.contains_key("HOME")
        && let Some(home) = home
    {
        command.env("HOME", home);
    }
    let mut child = command
        .args(["acp", "--agent-engine=v3", "--auth-method=cli"])
        .spawn()
        .map_err(|_| ())?;
    let mut input = child.stdin.take().ok_or(())?;
    let mut lines = BufReader::new(child.stdout.take().ok_or(())?).lines();
    input.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1,\"clientInfo\":{\"name\":\"sessiondock-quota\",\"version\":\"1\"},\"clientCapabilities\":{}}}\n").await.map_err(|_| ())?;
    let result = async {
        loop {
            let line = lines.next_line().await.map_err(|_| ())?.ok_or(())?;
            // Notifications and log lines do not answer either request.
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value.get("method").is_some() {
                continue;
            }
            if value["id"] == 1 {
                if value.get("error").is_some() {
                    return Err(());
                }
                input.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"_kiro/account/getUsage\",\"params\":{}}\n").await.map_err(|_| ())?;
            }
            if value["id"] == 2 {
                return kiro_snapshot(&value).ok_or(());
            }
        }
    }
    .await;
    drop(input);
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}
fn kiro_snapshot(response: &Value) -> Option<Value> {
    let result = &response["result"];
    if response.get("error").is_some() || result["success"] != true {
        return None;
    }
    let data = &result["data"];
    // The native reply carries only the UTC reset date of the billing cycle.
    let reset = data["billingCycleReset"]
        .as_str()
        .and_then(|day| chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map(|time| json!(time.and_utc().to_rfc3339()))
        .unwrap_or(Value::Null);
    let plan = data["planName"].as_str().unwrap_or("Kiro");
    let mut buckets = Vec::new();
    for row in data["usageBreakdowns"].as_array()? {
        let (Some(used), Some(limit)) = (row["used"].as_f64(), row["limit"].as_f64()) else {
            continue;
        };
        let limited = row["hasLimit"] != false && limit > 0.;
        // Native `percentage` is floored to an integer; keep the exact ratio.
        let remaining = limited.then(|| (100. - used * 100. / limit).clamp(0., 100.));
        let kind = row["resourceType"].as_str().unwrap_or("usage");
        let mut value = bucket(
            &format!("kiro:{}", kind.to_ascii_lowercase()),
            plan,
            json!("monthly"),
            remaining,
            reset.clone(),
        );
        value["used"] = json!(used);
        value["limit"] = json!(limit);
        if let Some(unit) = row["displayName"].as_str() {
            value["unit"] = json!(unit);
        }
        if !limited {
            value["remaining_text"] = json!("不限量");
        }
        buckets.push(value);
    }
    Some(snapshot("kiro", buckets))
}
pub fn claude_snapshot(value: &Value) -> Option<Value> {
    let limits = value.get("rate_limits")?.as_object()?;
    let mut buckets = Vec::new();
    for window in ["five_hour", "seven_day", "spend_limit"] {
        if let Some(b) = limits.get(window).filter(|b| b.is_object()) {
            buckets.push(bucket(
                window,
                "Claude Code",
                json!(window),
                b["used_percentage"].as_f64().map(|v| 100. - v),
                epoch(&b["resets_at"]),
            ));
        }
    }
    if buckets.is_empty() {
        None
    } else {
        Some(snapshot("claude", buckets))
    }
}
