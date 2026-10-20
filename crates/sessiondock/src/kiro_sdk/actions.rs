use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::transport::{Session, SpawnedSession, now_secs};
use super::{Bridge, CachedModels};

pub(super) enum KiroDiskSession {
    V3Dir {
        dir: PathBuf,
        session_json: PathBuf,
        events_jsonl: PathBuf,
    },
    V2File {
        json_path: PathBuf,
    },
}

pub(super) fn locate_kiro_session(root: &Path, sid_or_uid: &str) -> Option<KiroDiskSession> {
    let (path, _sid) =
        crate::sessions::index::summary::kiro::locate_session_file_by_uid_or_sid(root, sid_or_uid)?;
    if path.file_name().and_then(|n| n.to_str()) == Some("session.json") {
        let dir = path.parent().unwrap_or(root).to_path_buf();
        let events_jsonl = if dir.join("messages.jsonl").is_file() {
            dir.join("messages.jsonl")
        } else if dir.join("events.jsonl").is_file() {
            dir.join("events.jsonl")
        } else {
            dir.join("messages.jsonl")
        };
        Some(KiroDiskSession::V3Dir {
            dir,
            session_json: path,
            events_jsonl,
        })
    } else {
        Some(KiroDiskSession::V2File { json_path: path })
    }
}

pub(super) fn read_disk_session_metadata(root: &Path, sid: &str) -> Option<Value> {
    match locate_kiro_session(root, sid)? {
        KiroDiskSession::V3Dir { session_json, .. } => {
            let text = std::fs::read_to_string(session_json).ok()?;
            serde_json::from_str::<Value>(&text).ok()
        }
        KiroDiskSession::V2File { json_path } => {
            let text = std::fs::read_to_string(json_path).ok()?;
            serde_json::from_str::<Value>(&text).ok()
        }
    }
}

fn is_pid_alive(proc_root: &Path, pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let proc_dir = proc_root.join(pid.to_string());
    if !proc_dir.exists() {
        return false;
    }
    if let Ok(stat) = std::fs::read_to_string(proc_dir.join("stat")) {
        if let Some(rparen) = stat.rfind(')') {
            let rest = stat[rparen + 1..].trim_start();
            if rest.starts_with('Z') || rest.starts_with('X') {
                return false;
            }
        }
    }
    true
}

fn parse_lock_pid(content: &str) -> Option<u32> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(pid) = trimmed.parse::<u32>() {
        return Some(pid);
    }
    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        if let Some(pid) = val["pid"].as_u64() {
            return u32::try_from(pid).ok();
        }
        if let Some(s) = val["pid"].as_str() {
            return s.trim().parse::<u32>().ok();
        }
    }
    None
}

pub(super) fn build_prompt_blocks(
    text: &str,
    attachments: &[Value],
    quotes: &[Value],
) -> (Vec<Value>, String) {
    let mut blocks = Vec::new();
    let mut display_parts = Vec::new();

    for q in quotes {
        let qtext = q["text"]
            .as_str()
            .or_else(|| q["content"].as_str())
            .unwrap_or("")
            .trim();
        let path = q["path"].as_str().unwrap_or("");
        if !qtext.is_empty() {
            let formatted = if !path.is_empty() {
                format!("> [{path}]\n> {}", qtext.replace('\n', "\n> "))
            } else {
                format!("> {}", qtext.replace('\n', "\n> "))
            };
            blocks.push(json!({
                "type": "text",
                "text": formatted,
            }));
            display_parts.push(formatted);
        }
    }

    for att in attachments {
        let kind = att["kind"]
            .as_str()
            .or_else(|| att["type"].as_str())
            .unwrap_or("");
        let mime = att["mime"]
            .as_str()
            .or_else(|| att["mimeType"].as_str())
            .or_else(|| att["media_type"].as_str())
            .unwrap_or("image/png");
        let data_url = att["data_url"]
            .as_str()
            .or_else(|| att["data"].as_str())
            .unwrap_or("");
        if kind == "image" || mime.starts_with("image/") || data_url.starts_with("data:image/") {
            let b64 = if let Some((_, b)) = data_url.split_once(";base64,") {
                b
            } else {
                data_url
            };
            if !b64.is_empty() {
                blocks.push(json!({
                    "type": "image",
                    "mimeType": mime,
                    "data": b64,
                }));
            }
        } else if let Some(path) = att["path"].as_str() {
            blocks.push(json!({
                "type": "resource_link",
                "uri": format!("file://{path}"),
                "name": path,
            }));
        }
    }

    if !text.is_empty() || blocks.is_empty() {
        blocks.push(json!({
            "type": "text",
            "text": text,
        }));
    }
    if !text.is_empty() {
        display_parts.push(text.to_string());
    }
    let display_text = if display_parts.is_empty() {
        text.to_string()
    } else {
        display_parts.join("\n\n")
    };
    (blocks, display_text)
}

pub(super) fn extract_models_from_config_options(
    config_options: &[Value],
    v2_models: &Value,
) -> (Vec<Value>, Option<String>, Option<String>) {
    let mut models_out = Vec::new();
    let mut current_model = None;
    let mut current_effort = None;
    let mut efforts_list: Vec<String> = Vec::new();
    let mut default_effort: Option<String> = None;

    for opt in config_options {
        let id = opt["id"]
            .as_str()
            .or_else(|| opt["configId"].as_str())
            .unwrap_or("");
        let cat = opt["category"].as_str().unwrap_or("");
        if id.eq_ignore_ascii_case("effortLevel")
            || id.eq_ignore_ascii_case("effort")
            || id.eq_ignore_ascii_case("reasoningEffort")
            || cat.eq_ignore_ascii_case("thought_level")
            || cat.eq_ignore_ascii_case("effort")
        {
            if let Some(cur) = opt["currentValue"]
                .as_str()
                .or_else(|| opt["value"].as_str())
                .filter(|s| !s.trim().is_empty())
            {
                current_effort = Some(cur.to_string());
                default_effort = Some(cur.to_string());
            }
            if let Some(arr) = opt["options"].as_array() {
                for item in arr {
                    if let Some(val) = item["value"]
                        .as_str()
                        .or_else(|| item["id"].as_str())
                        .or_else(|| item.as_str())
                        .filter(|s| !s.trim().is_empty())
                    {
                        if !efforts_list.contains(&val.to_string()) {
                            efforts_list.push(val.to_string());
                        }
                    }
                }
            }
        }
    }

    for opt in config_options {
        let id = opt["id"]
            .as_str()
            .or_else(|| opt["configId"].as_str())
            .unwrap_or("");
        let cat = opt["category"].as_str().unwrap_or("");
        if id.eq_ignore_ascii_case("model")
            || id.eq_ignore_ascii_case("modelId")
            || cat.eq_ignore_ascii_case("model")
        {
            if let Some(cur) = opt["currentValue"]
                .as_str()
                .or_else(|| opt["value"].as_str())
                .filter(|s| !s.trim().is_empty())
            {
                current_model = Some(cur.to_string());
            }
            if let Some(arr) = opt["options"].as_array() {
                for item in arr {
                    let Some(mid) = item["value"]
                        .as_str()
                        .or_else(|| item["id"].as_str())
                        .or_else(|| item["modelId"].as_str())
                        .or_else(|| item.as_str())
                        .filter(|s| !s.trim().is_empty())
                    else {
                        continue;
                    };
                    let name = item["name"]
                        .as_str()
                        .or_else(|| item["label"].as_str())
                        .unwrap_or(mid);
                    let item_efforts: Vec<String> = item["efforts"]
                        .as_array()
                        .or_else(|| item["supportedReasoningEfforts"].as_array())
                        .map(|ea| {
                            ea.iter()
                                .filter_map(|e| {
                                    e.as_str()
                                        .or_else(|| e["id"].as_str())
                                        .or_else(|| e["reasoningEffort"].as_str())
                                        .map(str::to_string)
                                })
                                .collect()
                        })
                        .unwrap_or_else(|| efforts_list.clone());
                    let mut row = json!({
                        "id": mid,
                        "name": name,
                        "displayName": name,
                        "efforts": item_efforts,
                        "supportedReasoningEfforts": item_efforts,
                    });
                    if let Some(desc) = item["description"].as_str() {
                        row["description"] = json!(desc);
                    }
                    if let Some(ref de) = default_effort {
                        row["default_effort"] = json!(de);
                        row["defaultReasoningEffort"] = json!(de);
                    }
                    models_out.push(row);
                }
            }
        }
    }

    if models_out.is_empty() {
        if current_model.is_none() {
            current_model = v2_models["currentModelId"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string);
        }
        if let Some(avail) = v2_models["availableModels"]
            .as_array()
            .or_else(|| v2_models.as_array())
        {
            for m in avail {
                let Some(mid) = m["modelId"]
                    .as_str()
                    .or_else(|| m["id"].as_str())
                    .or_else(|| m["value"].as_str())
                    .filter(|s| !s.trim().is_empty())
                else {
                    continue;
                };
                let name = m["name"]
                    .as_str()
                    .or_else(|| m["displayName"].as_str())
                    .unwrap_or(mid);
                let item_efforts = if efforts_list.is_empty() {
                    vec![
                        "low".to_string(),
                        "medium".to_string(),
                        "high".to_string(),
                        "max".to_string(),
                    ]
                } else {
                    efforts_list.clone()
                };
                models_out.push(json!({
                    "id": mid,
                    "name": name,
                    "displayName": name,
                    "efforts": item_efforts,
                    "supportedReasoningEfforts": item_efforts,
                }));
            }
        }
    }

    (models_out, current_model, current_effort)
}

fn default_kiro_models() -> Vec<Value> {
    let efforts = vec!["low", "medium", "high", "max"];
    vec![
        json!({
            "id": "auto",
            "name": "Auto",
            "displayName": "Auto",
            "efforts": efforts,
            "supportedReasoningEfforts": efforts,
            "default_effort": "medium",
            "defaultReasoningEffort": "medium",
        }),
        json!({
            "id": "claude-sonnet-4.5",
            "name": "Claude Sonnet 4.5",
            "displayName": "Claude Sonnet 4.5",
            "efforts": efforts,
            "supportedReasoningEfforts": efforts,
            "default_effort": "medium",
            "defaultReasoningEffort": "medium",
        }),
        json!({
            "id": "claude-sonnet-4",
            "name": "Claude Sonnet 4",
            "displayName": "Claude Sonnet 4",
            "efforts": efforts,
            "supportedReasoningEfforts": efforts,
            "default_effort": "medium",
            "defaultReasoningEffort": "medium",
        }),
        json!({
            "id": "claude-haiku-4.5",
            "name": "Claude Haiku 4.5",
            "displayName": "Claude Haiku 4.5",
            "efforts": efforts,
            "supportedReasoningEfforts": efforts,
            "default_effort": "low",
            "defaultReasoningEffort": "low",
        }),
        json!({
            "id": "claude-opus-4.5",
            "name": "Claude Opus 4.5",
            "displayName": "Claude Opus 4.5",
            "efforts": efforts,
            "supportedReasoningEfforts": efforts,
            "default_effort": "high",
            "defaultReasoningEffort": "high",
        }),
    ]
}

fn ensure_model_in_list(list: &mut Vec<Value>, model_id: Option<&str>) {
    let Some(mid) = model_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let exists = list
        .iter()
        .any(|item| item["id"].as_str() == Some(mid) || item["name"].as_str() == Some(mid));
    if !exists {
        let efforts = list
            .first()
            .and_then(|first| first["efforts"].as_array())
            .cloned()
            .unwrap_or_else(|| vec![json!("low"), json!("medium"), json!("high"), json!("max")]);
        list.insert(
            0,
            json!({
                "id": mid,
                "name": mid,
                "displayName": mid,
                "efforts": efforts,
                "supportedReasoningEfforts": efforts,
            }),
        );
    }
}

impl Bridge {
    pub(super) fn check_conflict(&self, sid: &str, own_pid: Option<u32>) -> Result<(), String> {
        if sid.is_empty() {
            return Ok(());
        }
        // 1. Check Kiro .lock files (<root>/cli/<sid>.lock, <root>/<sid>.lock, etc.)
        let mut lock_candidates = vec![
            self.root.join("cli").join(format!("{sid}.lock")),
            self.root.join(format!("{sid}.lock")),
            self.root.join("cli").join(sid).join("session.lock"),
            self.root.join(sid).join("session.lock"),
        ];
        if let Some(disk_loc) = locate_kiro_session(&self.root, sid) {
            match disk_loc {
                KiroDiskSession::V3Dir { dir, .. } => {
                    lock_candidates.push(dir.join("session.lock"));
                    lock_candidates.push(dir.with_extension("lock"));
                }
                KiroDiskSession::V2File { json_path } => {
                    lock_candidates.push(json_path.with_extension("lock"));
                }
            }
        }
        for lock_path in &lock_candidates {
            if !lock_path.exists() {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(lock_path) {
                if let Some(pid) = parse_lock_pid(&content) {
                    if own_pid == Some(pid) {
                        continue;
                    }
                    if is_pid_alive(&self.proc_root, pid) {
                        return Err(
                            "kiro_sdk_session_busy: 该 Kiro 会话正在 CLI 终端中运行，请先停止 CLI 终端再启动 SDK 模式。"
                                .into(),
                        );
                    }
                }
            }
            // Stale lock file: clean up so session/load can proceed.
            let _ = std::fs::remove_file(lock_path);
        }

        // 2. Check /proc scan for live external CLI holding this session
        let tree = Arc::new(crate::runtime::procscan::ProcTree::open(
            self.proc_root.clone(),
        ));
        let roots = crate::runtime::procscan::SessionRoots::new([self.root.as_path()])
            .with_kiro_root(Some(self.root.clone()));
        let scan = crate::runtime::procscan::scan(tree.clone(), None, &roots);
        let is_ours = |pid: i64| -> bool {
            let pid = pid.unsigned_abs() as u32;
            own_pid.is_some_and(|own| pid == own || tree.cli_ancestor(pid) == Some(own))
        };
        let sid_lower = sid.to_ascii_lowercase();
        if scan
            .sids
            .get(&sid_lower)
            .or_else(|| scan.sids.get(sid))
            .is_some_and(|pids| pids.iter().any(|pid| !is_ours(*pid)))
        {
            return Err(
                "kiro_sdk_session_busy: 该 Kiro 会话正在 CLI 终端中运行，请先停止 CLI 终端再启动 SDK 模式。"
                    .into(),
            );
        }
        if let Some(disk_loc) = locate_kiro_session(&self.root, sid) {
            let prefix = match disk_loc {
                KiroDiskSession::V3Dir { dir, .. } => format!("{}/", dir.display()),
                KiroDiskSession::V2File { json_path } => json_path.to_string_lossy().to_string(),
            };
            if scan
                .paths
                .iter()
                .any(|(file, pids)| file.starts_with(&prefix) && pids.iter().any(|p| !is_ours(*p)))
            {
                return Err(
                    "kiro_sdk_session_busy: 该 Kiro 会话正在 CLI 终端中运行，请先停止 CLI 终端再启动 SDK 模式。"
                        .into(),
                );
            }
        }
        Ok(())
    }

    pub(super) async fn apply_setting(
        &self,
        session: &Arc<Session>,
        kind: &str,
        val: &str,
    ) -> Result<(), String> {
        let sid = session.session_id.lock().unwrap().clone();
        if sid.is_empty() {
            return Err("session_id not initialized".into());
        }
        match kind {
            "model" => {
                let config_id = {
                    let st = session.state.lock().unwrap();
                    st.config_options
                        .iter()
                        .find_map(|opt| {
                            let id = opt["id"].as_str().or_else(|| opt["configId"].as_str())?;
                            let cat = opt["category"].as_str().unwrap_or("");
                            if id.eq_ignore_ascii_case("model")
                                || id.eq_ignore_ascii_case("modelId")
                                || cat.eq_ignore_ascii_case("model")
                            {
                                Some(id.to_string())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| "model".to_string())
                };
                let res = session
                    .request(
                        "session/set_config_option",
                        json!({
                            "sessionId": sid,
                            "configId": config_id,
                            "value": val,
                        }),
                    )
                    .await;
                match res {
                    Ok(body) => {
                        let mut st = session.state.lock().unwrap();
                        if let Some(arr) = body
                            .get("configOptions")
                            .or_else(|| body.get("options"))
                            .and_then(|v| v.as_array())
                        {
                            st.apply_config_options(arr);
                        }
                        st.model = Some(val.to_string());
                        st.model_observed = false;
                        st.update_config_option_value(&config_id, val);
                        st.publish();
                    }
                    Err(_) => {
                        let v2 = session
                            .request(
                                "session/set_model",
                                json!({
                                    "sessionId": sid,
                                    "modelId": val,
                                }),
                            )
                            .await;
                        if v2.is_err() {
                            let _ = session
                                .request(
                                    "_kiro.dev/commands/execute",
                                    json!({
                                        "sessionId": sid,
                                        "command": "model",
                                        "args": { "modelId": val, "model": val },
                                    }),
                                )
                                .await;
                        }
                        let mut st = session.state.lock().unwrap();
                        st.model = Some(val.to_string());
                        st.model_observed = false;
                        st.update_config_option_value(&config_id, val);
                        st.publish();
                    }
                }
            }
            "effort" | "effortLevel" => {
                let config_id = {
                    let st = session.state.lock().unwrap();
                    st.config_options
                        .iter()
                        .find_map(|opt| {
                            let id = opt["id"].as_str().or_else(|| opt["configId"].as_str())?;
                            let cat = opt["category"].as_str().unwrap_or("");
                            if id.eq_ignore_ascii_case("effortLevel")
                                || id.eq_ignore_ascii_case("effort")
                                || id.eq_ignore_ascii_case("reasoningEffort")
                                || cat.eq_ignore_ascii_case("thought_level")
                                || cat.eq_ignore_ascii_case("effort")
                            {
                                Some(id.to_string())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| "effortLevel".to_string())
                };
                if let Ok(body) = session
                    .request(
                        "session/set_config_option",
                        json!({
                            "sessionId": sid,
                            "configId": config_id,
                            "value": val,
                        }),
                    )
                    .await
                {
                    let mut st = session.state.lock().unwrap();
                    if let Some(arr) = body
                        .get("configOptions")
                        .or_else(|| body.get("options"))
                        .and_then(|v| v.as_array())
                    {
                        st.apply_config_options(arr);
                    }
                } else {
                    let _ = session
                        .request(
                            "_kiro.dev/commands/execute",
                            json!({
                                "sessionId": sid,
                                "command": "effort",
                                "args": { "effortLevel": val, "effort": val },
                            }),
                        )
                        .await;
                }
                let mut st = session.state.lock().unwrap();
                st.effort = if val.is_empty() {
                    None
                } else {
                    Some(val.to_string())
                };
                st.update_config_option_value(&config_id, val);
                st.publish();
            }
            "mode" => {
                let has_v3_mode = {
                    let st = session.state.lock().unwrap();
                    st.config_options.iter().any(|opt| {
                        let id = opt["id"]
                            .as_str()
                            .or_else(|| opt["configId"].as_str())
                            .unwrap_or("");
                        let cat = opt["category"].as_str().unwrap_or("");
                        id.eq_ignore_ascii_case("mode") || cat.eq_ignore_ascii_case("mode")
                    })
                };
                let mut v3_succeeded = false;
                if has_v3_mode {
                    if let Ok(body) = session
                        .request(
                            "session/set_config_option",
                            json!({
                                "sessionId": sid,
                                "configId": "mode",
                                "value": val,
                            }),
                        )
                        .await
                    {
                        v3_succeeded = true;
                        let mut st = session.state.lock().unwrap();
                        if let Some(arr) = body
                            .get("configOptions")
                            .or_else(|| body.get("options"))
                            .and_then(|v| v.as_array())
                        {
                            st.apply_config_options(arr);
                        }
                    }
                }
                if !v3_succeeded {
                    let _ = session
                        .request(
                            "session/set_mode",
                            json!({
                                "sessionId": sid,
                                "modeId": val,
                            }),
                        )
                        .await;
                }
                let mut st = session.state.lock().unwrap();
                st.mode = Some(val.to_string());
                if st.modes.is_object() {
                    st.modes["currentModeId"] = json!(val);
                }
                st.update_config_option_value("mode", val);
                st.publish();
            }
            "permission_mode" | "autopilot" => {
                let enabled = matches!(
                    val.trim().to_ascii_lowercase().as_str(),
                    "true"
                        | "1"
                        | "on"
                        | "enabled"
                        | "autopilot"
                        | "auto"
                        | "always"
                        | "bypasspermissions"
                );
                let (config_id, wire_val) = {
                    let st = session.state.lock().unwrap();
                    let found = st.config_options.iter().find_map(|opt| {
                        let id = opt["id"].as_str().or_else(|| opt["configId"].as_str())?;
                        let cat = opt["category"].as_str().unwrap_or("");
                        if id.eq_ignore_ascii_case("autopilot")
                            || id.eq_ignore_ascii_case("approval")
                            || id.eq_ignore_ascii_case("permissionMode")
                            || cat.eq_ignore_ascii_case("autopilot")
                            || cat.eq_ignore_ascii_case("approval")
                        {
                            let uses_on_off = opt["options"]
                                .as_array()
                                .map(|arr| {
                                    arr.iter().any(|o| {
                                        o["value"].as_str() == Some("on")
                                            || o["value"].as_str() == Some("off")
                                    })
                                })
                                .unwrap_or(false);
                            let uses_bool_str = opt["options"]
                                .as_array()
                                .map(|arr| {
                                    arr.iter().any(|o| {
                                        o["value"].as_str() == Some("true")
                                            || o["value"].as_str() == Some("false")
                                    })
                                })
                                .unwrap_or(!uses_on_off);
                            let wv = if uses_on_off {
                                if enabled { "on" } else { "off" }
                            } else if uses_bool_str {
                                if enabled { "true" } else { "false" }
                            } else if enabled {
                                "autopilot"
                            } else {
                                "default"
                            };
                            Some((id.to_string(), wv.to_string()))
                        } else {
                            None
                        }
                    });
                    found.unwrap_or_else(|| {
                        (
                            "autopilot".to_string(),
                            if enabled { "true" } else { "false" }.to_string(),
                        )
                    })
                };
                if let Ok(body) = session
                    .request(
                        "session/set_config_option",
                        json!({
                            "sessionId": sid,
                            "configId": config_id,
                            "value": wire_val,
                        }),
                    )
                    .await
                {
                    let mut st = session.state.lock().unwrap();
                    if let Some(arr) = body
                        .get("configOptions")
                        .or_else(|| body.get("options"))
                        .and_then(|v| v.as_array())
                    {
                        st.apply_config_options(arr);
                    }
                }
                let mut st = session.state.lock().unwrap();
                st.autopilot = Some(enabled);
                st.permission_mode = Some(if enabled { "autopilot" } else { "default" }.into());
                st.update_config_option_value(&config_id, &wire_val);
                st.publish();
            }
            other => {
                if let Ok(body) = session
                    .request(
                        "session/set_config_option",
                        json!({
                            "sessionId": sid,
                            "configId": other,
                            "value": val,
                        }),
                    )
                    .await
                {
                    let mut st = session.state.lock().unwrap();
                    if let Some(arr) = body
                        .get("configOptions")
                        .or_else(|| body.get("options"))
                        .and_then(|v| v.as_array())
                    {
                        st.apply_config_options(arr);
                    }
                    st.update_config_option_value(other, val);
                    st.publish();
                }
            }
        }
        Ok(())
    }

    pub(super) async fn models_action(
        &self,
        uid: &str,
        cwd: &str,
        target: &Value,
    ) -> Result<Value, String> {
        let observed_from_target = target["observed_model"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        // 1. If session is already running in memory, read its live configOptions/models
        let existing_session = if !uid.is_empty() {
            self.sessions.lock().unwrap().get(uid).cloned()
        } else {
            None
        };
        if let Some(session) = existing_session {
            let (mut list, mut cur_model, mut model_observed, mut cur_effort, config_options) = {
                let st = session.state.lock().unwrap();
                let (list, extracted_model, extracted_effort) =
                    extract_models_from_config_options(&st.config_options, &st.models);
                (
                    list,
                    st.model.clone().or(extracted_model),
                    st.model_observed,
                    st.effort.clone().or(extracted_effort),
                    st.config_options.clone(),
                )
            };

            if cur_model.is_none() {
                let sid = self.resolve_sid(uid, target, &Value::Null);
                if let Some(disk) = read_disk_session_metadata(&self.root, &sid) {
                    cur_model = disk["modelId"]
                        .as_str()
                        .or_else(|| disk["model"].as_str())
                        .or_else(|| disk["currentModelId"].as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    if cur_effort.is_none() {
                        cur_effort = disk["effortLevel"]
                            .as_str()
                            .or_else(|| disk["effort"].as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string);
                    }
                    if cur_model.is_some() {
                        model_observed = true;
                    }
                }
                if cur_model.is_none() && observed_from_target.is_some() {
                    cur_model = observed_from_target;
                    model_observed = true;
                }
            }

            if list.is_empty() {
                let (catalog_list, default_m) = self.catalog_or_probe_models(cwd).await;
                list = catalog_list;
                if cur_model.is_none() {
                    cur_model = default_m;
                }
            }
            ensure_model_in_list(&mut list, cur_model.as_deref());
            return Ok(json!({
                "models": list,
                "current_model": cur_model,
                "model": cur_model,
                "model_observed": model_observed,
                "effort": cur_effort,
                "configOptions": config_options,
            }));
        }

        // 2. If uid is provided but SDK process is not yet started, read disk session.json / .json
        let (mut list, default_model) = self.catalog_or_probe_models(cwd).await;
        if !uid.is_empty() {
            let sid = self.resolve_sid(uid, target, &Value::Null);
            let mut disk_model = None;
            let mut disk_effort = None;
            if let Some(disk) = read_disk_session_metadata(&self.root, &sid) {
                disk_model = disk["modelId"]
                    .as_str()
                    .or_else(|| disk["model"].as_str())
                    .or_else(|| disk["currentModelId"].as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                disk_effort = disk["effortLevel"]
                    .as_str()
                    .or_else(|| disk["effort"].as_str())
                    .or_else(|| disk["reasoningEffort"].as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                if let Some(arr) = disk.get("configOptions").and_then(|v| v.as_array()) {
                    let (d_list, dm, de) = extract_models_from_config_options(arr, &Value::Null);
                    if !d_list.is_empty() {
                        list = d_list;
                    }
                    if disk_model.is_none() {
                        disk_model = dm;
                    }
                    if disk_effort.is_none() {
                        disk_effort = de;
                    }
                }
            }
            if disk_model.is_none() {
                disk_model = observed_from_target;
            }
            let model_observed = disk_model.is_some();
            let chosen_model = disk_model.or(default_model);
            ensure_model_in_list(&mut list, chosen_model.as_deref());
            return Ok(json!({
                "models": list,
                "current_model": chosen_model,
                "model": chosen_model,
                "model_observed": model_observed,
                "effort": disk_effort,
            }));
        }

        // 3. UID-less models request for new-session modal picker
        ensure_model_in_list(&mut list, default_model.as_deref());
        Ok(json!({
            "models": list,
            "default_model": default_model,
            "current_model": default_model,
            "model": default_model,
        }))
    }

    pub(super) async fn catalog_or_probe_models(&self, cwd: &str) -> (Vec<Value>, Option<String>) {
        // First check if any active session already has config_options/models
        {
            let sessions: Vec<Arc<Session>> =
                self.sessions.lock().unwrap().values().cloned().collect();
            for s in sessions {
                let st = s.state.lock().unwrap();
                let (list, cur, _) =
                    extract_models_from_config_options(&st.config_options, &st.models);
                if !list.is_empty() {
                    return (list, cur.or_else(|| st.model.clone()));
                }
            }
        }

        if let Some(cached) = self.models_cache.lock().unwrap().as_ref()
            && cached.at.elapsed() < Duration::from_secs(600)
            && !cached.models.is_empty()
        {
            return (cached.models.clone(), cached.default_model.clone());
        }

        let cat = crate::lifecycle::models::catalog(&self.profile);
        if !cat.models.is_empty() {
            let list: Vec<Value> = cat
                .models
                .iter()
                .map(|m| {
                    let mut row = json!({
                        "id": m.id,
                        "name": m.name,
                        "displayName": m.name,
                        "efforts": m.efforts,
                        "supportedReasoningEfforts": m.efforts,
                    });
                    if let Some(ref de) = m.default_effort {
                        row["default_effort"] = json!(de);
                        row["defaultReasoningEffort"] = json!(de);
                    }
                    row
                })
                .collect();
            *self.models_cache.lock().unwrap() = Some(CachedModels {
                at: Instant::now(),
                models: list.clone(),
                default_model: cat.default_model.clone(),
            });
            return (list, cat.default_model);
        }

        if let Ok((list, default_model)) = self.probe_models(cwd).await
            && !list.is_empty()
        {
            *self.models_cache.lock().unwrap() = Some(CachedModels {
                at: Instant::now(),
                models: list.clone(),
                default_model: default_model.clone(),
            });
            return (list, default_model);
        }

        let fallback = default_kiro_models();
        let default_m = Some("auto".to_string());
        (fallback, default_m)
    }

    async fn probe_models(&self, cwd: &str) -> Result<(Vec<Value>, Option<String>), String> {
        let probe_dir = std::env::temp_dir().join(format!(
            "sessiondock-kiro-probe-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let _ = std::fs::create_dir_all(&probe_dir);
        let probe_cwd = if !cwd.is_empty() && Path::new(cwd).is_dir() {
            cwd.to_string()
        } else {
            probe_dir.to_string_lossy().to_string()
        };
        let mut cmd = self.command(&probe_cwd);
        cmd.env("HOME", &probe_dir);
        let token = CancellationToken::new();
        let spawned = Session::spawn_or_attach(None, None, "", "", &probe_cwd, cmd, token).await;
        let Ok(SpawnedSession::Fresh(session)) = spawned else {
            let _ = std::fs::remove_dir_all(&probe_dir);
            return Err("probe spawn failed".into());
        };
        let _ = session
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": { "name": "sessiondock", "version": env!("CARGO_PKG_VERSION") },
                    "clientCapabilities": {
                        "_meta": { "kiro": { "userInput": true } }
                    }
                }),
            )
            .await;
        let res = session
            .request(
                "session/new",
                json!({
                    "cwd": probe_cwd,
                    "mcpServers": [],
                }),
            )
            .await;
        session.kill().await;
        let _ = std::fs::remove_dir_all(&probe_dir);
        let res = res?;
        let config_opts = res
            .get("configOptions")
            .or_else(|| res.get("options"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let v2_models = res.get("models").cloned().unwrap_or(Value::Null);
        let (list, cur, _) = extract_models_from_config_options(&config_opts, &v2_models);
        Ok((list, cur))
    }

    pub(super) async fn respond(&self, uid: &str, payload: &Value) -> Result<Value, String> {
        let session = self.get_session(uid)?;
        let key = payload["key"]
            .as_str()
            .ok_or("missing request key")?
            .to_string();
        let result = payload.get("result").cloned().unwrap_or(Value::Null);
        let pending = {
            let mut st = session.state.lock().unwrap();
            let req = st
                .requests
                .remove(&key)
                .ok_or_else(|| format!("request {key} not found"))?;
            st.publish();
            req
        };
        let rpc_res = if key.starts_with("perm-") {
            if result.as_str() == Some("cancelled")
                || result["outcome"].as_str() == Some("cancelled")
                || result["behavior"].as_str() == Some("cancel")
            {
                json!({
                    "outcome": { "outcome": "cancelled" }
                })
            } else {
                let option_id = result
                    .as_str()
                    .or_else(|| result["optionId"].as_str())
                    .or_else(|| result["id"].as_str())
                    .or_else(|| result["value"].as_str())
                    .or_else(|| result["outcome"]["optionId"].as_str())
                    .unwrap_or("allow_once");
                json!({
                    "outcome": {
                        "outcome": "selected",
                        "optionId": option_id,
                    }
                })
            }
        } else if result["behavior"].as_str() == Some("deny")
            || result["action"].as_str() == Some("decline")
            || result["action"].as_str() == Some("cancel")
        {
            let action = result["action"].as_str().unwrap_or("decline");
            json!({
                "action": action,
                "cancelled": true,
                "answers": {},
                "answer": Value::Null,
            })
        } else {
            let mut answer_str = String::new();
            let mut answers_map = serde_json::Map::new();
            if let Some(s) = result.as_str() {
                answer_str = s.to_string();
                answers_map.insert("answer".into(), json!(s));
            } else if let Some(ans_obj) = result.get("answers").and_then(|v| v.as_object()) {
                answers_map = ans_obj.clone();
                if let Some((_, first_val)) = ans_obj.iter().next() {
                    if let Some(s) = first_val.as_str() {
                        answer_str = s.to_string();
                    } else if let Some(arr) = first_val.as_array() {
                        let joined: Vec<String> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect();
                        answer_str = joined.join(", ");
                    } else {
                        answer_str = first_val.to_string();
                    }
                }
            } else if let Some(ans_val) = result.get("answer") {
                if let Some(s) = ans_val.as_str() {
                    answer_str = s.to_string();
                    answers_map.insert("answer".into(), json!(s));
                } else if let Some(obj) = ans_val.as_object() {
                    answers_map = obj.clone();
                    if let Some((_, first_val)) = obj.iter().next() {
                        answer_str = first_val
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| first_val.to_string());
                    }
                }
            } else if let Some(content_obj) = result.get("content").and_then(|v| v.as_object()) {
                answers_map = content_obj.clone();
                if let Some((_, first_val)) = content_obj.iter().next() {
                    answer_str = first_val
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| first_val.to_string());
                }
            }
            let content_val = if let Some(c) = result.get("content") {
                c.clone()
            } else if !answers_map.is_empty() {
                Value::Object(answers_map.clone())
            } else {
                json!({ "answer": answer_str })
            };
            json!({
                "action": "accept",
                "answer": answer_str,
                "value": answer_str,
                "text": answer_str,
                "answers": Value::Object(answers_map),
                "content": content_val,
            })
        };
        session.respond_rpc(pending.rpc_id, rpc_res).await?;
        Ok(session.snapshot())
    }

    pub(super) async fn steer(&self, uid: &str, payload: &Value) -> Result<Value, String> {
        let session = self.get_session(uid)?;
        let text = payload["text"].as_str().unwrap_or("").trim().to_string();
        let attachments = payload["attachments"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let quotes = payload["quotes"].as_array().cloned().unwrap_or_default();
        if text.is_empty() && attachments.is_empty() && quotes.is_empty() {
            return Err("steer text is empty".into());
        }
        let sid = session.session_id.lock().unwrap().clone();
        {
            let st = session.state.lock().unwrap();
            if !st.running {
                return Err("session is not currently running a turn; use turn instead".into());
            }
        }
        let (content, display_text) = build_prompt_blocks(&text, &attachments, &quotes);
        {
            let mut st = session.state.lock().unwrap();
            let user_id = st.alloc_item_id("user");
            st.items.push(json!({
                "id": user_id,
                "role": "user",
                "text": display_text,
                "content": display_text,
                "steered": true,
            }));
            st.current_assistant_id = None;
            st.current_thinking_id = None;
            st.publish();
        }
        let steer_params = json!({
            "sessionId": sid,
            "prompt": content,
            "text": display_text,
        });
        if session
            .request("_session/steer", steer_params.clone())
            .await
            .is_err()
        {
            let _ = session.request("session/steer", steer_params).await;
        }
        let mut snap = session.snapshot();
        snap["accepted"] = json!(true);
        Ok(snap)
    }

    pub(super) async fn handle_queue_action(
        &self,
        uid: &str,
        payload: &Value,
    ) -> Result<Value, String> {
        let action = payload["action"].as_str().unwrap_or("");
        if action == "steer" || payload["operation"].as_str() == Some("steer") {
            return self.steer(uid, payload).await;
        }
        let session = self.get_session(uid)?;
        Ok(session.snapshot())
    }

    pub(super) async fn compact(
        &self,
        uid: &str,
        cwd: &str,
        target: &Value,
    ) -> Result<Value, String> {
        let session = self.ensure_open(uid, cwd, target).await?;
        let sid = session.session_id.lock().unwrap().clone();
        {
            let mut st = session.state.lock().unwrap();
            st.compact = Some(json!({ "state": "compacting" }));
            st.status = Some("compacting".into());
            st.publish();
        }
        let mut res = session
            .request(
                "_kiro/session/compact",
                json!({
                    "sessionId": sid,
                }),
            )
            .await;
        if res.is_err() {
            res = session
                .request(
                    "_kiro.dev/commands/execute",
                    json!({
                        "sessionId": sid,
                        "command": "compact",
                        "args": {},
                    }),
                )
                .await;
        }
        if res.is_err() {
            res = session
                .request(
                    "session/compact",
                    json!({
                        "sessionId": sid,
                    }),
                )
                .await;
        }
        if res.is_err() {
            res = session
                .request(
                    "session/prompt",
                    json!({
                        "sessionId": sid,
                        "prompt": [{ "type": "text", "text": "/compact" }],
                    }),
                )
                .await;
        }
        match res {
            Ok(val) => {
                {
                    let mut st = session.state.lock().unwrap();
                    st.apply_metadata(&val);
                    st.compact = Some(json!({ "state": "success" }));
                    st.status = None;
                    st.publish();
                }
                Ok(session.snapshot())
            }
            Err(err) => {
                {
                    let mut st = session.state.lock().unwrap();
                    st.compact = Some(json!({
                        "state": "failed",
                        "error": err,
                    }));
                    st.status = None;
                    st.publish();
                }
                Err(format!("kiro_sdk_compact_failed: {err}"))
            }
        }
    }

    pub(super) async fn rename_session(
        &self,
        uid: &str,
        target: &Value,
        payload: &Value,
    ) -> Result<Value, String> {
        let sid = self.resolve_sid(uid, target, payload);
        let title = payload["title"]
            .as_str()
            .or_else(|| payload["name"].as_str())
            .unwrap_or("")
            .trim();
        if title.is_empty() {
            return Err("title is required".into());
        }

        if let Some(disk_loc) =
            locate_kiro_session(&self.root, &sid).or_else(|| locate_kiro_session(&self.root, uid))
        {
            let path = match disk_loc {
                KiroDiskSession::V3Dir { session_json, .. } => session_json,
                KiroDiskSession::V2File { json_path } => json_path,
            };
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("read session file failed: {e}"))?;
            let mut doc: Value = serde_json::from_str(&content)
                .map_err(|e| format!("parse session file failed: {e}"))?;
            if let Some(obj) = doc.as_object_mut() {
                obj.insert("title".into(), json!(title));
                if obj.contains_key("name") {
                    obj.insert("name".into(), json!(title));
                }
            }
            let updated = serde_json::to_string_pretty(&doc)
                .map_err(|e| format!("serialize session file failed: {e}"))?;
            std::fs::write(&path, updated)
                .map_err(|e| format!("write session file failed: {e}"))?;
        }

        if let Some(session) = self.sessions.lock().unwrap().get(uid).cloned() {
            let mut st = session.state.lock().unwrap();
            st.title = Some(title.to_string());
            st.publish();
        }

        Ok(json!({
            "uid": uid,
            "title": title,
        }))
    }

    pub(super) async fn fork_session(
        &self,
        uid: &str,
        cwd: &str,
        target: &Value,
        payload: &Value,
    ) -> Result<Value, String> {
        let sid = self.resolve_sid(uid, target, payload);
        let new_sid = generate_uuid_v4();
        let nth_user = payload
            .get("nth_user")
            .or_else(|| payload.get("turn_index"))
            .or_else(|| payload.get("prompt_index"))
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let dst_session_path = match locate_kiro_session(&self.root, &sid)
            .or_else(|| locate_kiro_session(&self.root, uid))
        {
            Some(KiroDiskSession::V3Dir {
                dir,
                session_json,
                events_jsonl,
            }) => {
                let parent_dir = dir.parent().unwrap_or(&self.root);
                let dst_dir = parent_dir.join(&new_sid);
                std::fs::create_dir_all(&dst_dir)
                    .map_err(|e| format!("create forked session dir failed: {e}"))?;

                let content = std::fs::read_to_string(&session_json)
                    .map_err(|e| format!("read source session.json failed: {e}"))?;
                let mut doc: Value = serde_json::from_str(&content)
                    .map_err(|e| format!("parse source session.json failed: {e}"))?;
                if let Some(obj) = doc.as_object_mut() {
                    obj.insert("sessionId".into(), json!(new_sid));
                    if obj.contains_key("id") {
                        obj.insert("id".into(), json!(new_sid));
                    }
                    if obj.contains_key("session_id") {
                        obj.insert("session_id".into(), json!(new_sid));
                    }
                    let old_title = obj
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Kiro Session");
                    let new_title = payload["title"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{old_title} (fork)"));
                    obj.insert("title".into(), json!(new_title));
                }
                let out_meta = serde_json::to_string_pretty(&doc)
                    .map_err(|e| format!("serialize forked session.json failed: {e}"))?;
                let dst_meta_path = dst_dir.join("session.json");
                std::fs::write(&dst_meta_path, out_meta)
                    .map_err(|e| format!("write forked session.json failed: {e}"))?;

                let jsonl_name = events_jsonl
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("messages.jsonl");
                if events_jsonl.is_file() {
                    let events_content = std::fs::read_to_string(&events_jsonl)
                        .map_err(|e| format!("read source {jsonl_name} failed: {e}"))?;
                    let mut out_lines = Vec::new();
                    let mut user_seen = 0usize;
                    let mut stop_after_assistant = false;

                    for line in events_content.lines() {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if let Ok(mut ev) = serde_json::from_str::<Value>(line) {
                            let is_user = is_v3_user_event(&ev);
                            if is_user {
                                if stop_after_assistant {
                                    break;
                                }
                                if let Some(target_idx) = nth_user
                                    && user_seen == target_idx
                                {
                                    stop_after_assistant = true;
                                }
                                user_seen += 1;
                            }
                            if let Some(obj) = ev.as_object_mut() {
                                if obj.get("sessionId").and_then(|v| v.as_str()) == Some(&sid) {
                                    obj.insert("sessionId".into(), json!(new_sid));
                                }
                                if obj.get("session_id").and_then(|v| v.as_str()) == Some(&sid) {
                                    obj.insert("session_id".into(), json!(new_sid));
                                }
                            }
                            if let Ok(s) = serde_json::to_string(&ev) {
                                out_lines.push(s);
                            }
                        } else {
                            out_lines.push(line.to_string());
                        }
                    }
                    let mut body = out_lines.join("\n");
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    std::fs::write(dst_dir.join(jsonl_name), body)
                        .map_err(|e| format!("write forked {jsonl_name} failed: {e}"))?;
                } else {
                    let _ = std::fs::write(dst_dir.join(jsonl_name), "");
                }
                dst_meta_path
            }
            Some(KiroDiskSession::V2File { json_path }) => {
                let parent_dir = json_path.parent().unwrap_or(&self.root);
                let dst_path = parent_dir.join(format!("{new_sid}.json"));
                let content = std::fs::read_to_string(&json_path)
                    .map_err(|e| format!("read source v2 json failed: {e}"))?;
                let mut doc: Value = serde_json::from_str(&content)
                    .map_err(|e| format!("parse source v2 json failed: {e}"))?;
                if let Some(obj) = doc.as_object_mut() {
                    obj.insert("session_id".into(), json!(new_sid));
                    if obj.contains_key("sessionId") {
                        obj.insert("sessionId".into(), json!(new_sid));
                    }
                    let old_title = obj
                        .get("title")
                        .or_else(|| obj.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Kiro Session");
                    let new_title = payload["title"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{old_title} (fork)"));
                    obj.insert("title".into(), json!(new_title));
                    if let Some(target_idx) = nth_user
                        && let Some(hist) = obj.get_mut("history").and_then(|v| v.as_array_mut())
                        && target_idx + 1 < hist.len()
                    {
                        hist.truncate(target_idx + 1);
                    }
                }
                let out_json = serde_json::to_string_pretty(&doc)
                    .map_err(|e| format!("serialize forked v2 json failed: {e}"))?;
                std::fs::write(&dst_path, out_json)
                    .map_err(|e| format!("write forked v2 json failed: {e}"))?;
                dst_path
            }
            None => {
                let dst_dir = self.root.join("cli").join(&new_sid);
                std::fs::create_dir_all(&dst_dir)
                    .map_err(|e| format!("create forked session dir failed: {e}"))?;
                let now_iso = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now())
                    .to_rfc3339();
                let doc = json!({
                    "sessionId": new_sid,
                    "title": payload["title"].as_str().unwrap_or("Kiro Session (fork)"),
                    "cwd": cwd,
                    "workspaceDirectory": cwd,
                    "createdAt": now_iso,
                    "updatedAt": now_iso,
                });
                let out_meta = serde_json::to_string_pretty(&doc)
                    .map_err(|e| format!("serialize forked session.json failed: {e}"))?;
                let dst_meta_path = dst_dir.join("session.json");
                std::fs::write(&dst_meta_path, out_meta)
                    .map_err(|e| format!("write forked session.json failed: {e}"))?;
                let _ = std::fs::write(dst_dir.join("messages.jsonl"), "");
                dst_meta_path
            }
        };

        let canon_path = dst_session_path
            .canonicalize()
            .unwrap_or_else(|_| dst_session_path.clone());
        let new_uid = crate::sessions::uid_for("kiro", &canon_path);

        Ok(json!({
            "uid": new_uid,
            "session_id": new_sid,
            "cwd": cwd,
            "session": {
                "uid": new_uid,
                "source": "kiro",
                "session_id": new_sid,
                "sid": new_sid,
                "cwd": cwd,
                "path": canon_path,
                "sdk": true,
            }
        }))
    }
}

fn is_v3_user_event(ev: &Value) -> bool {
    let role = ev["role"]
        .as_str()
        .or_else(|| ev["message"]["role"].as_str())
        .or_else(|| ev["data"]["role"].as_str())
        .unwrap_or("");
    if role == "user" {
        return true;
    }
    let kind = ev["type"]
        .as_str()
        .or_else(|| ev["kind"].as_str())
        .or_else(|| ev["event"].as_str())
        .or_else(|| ev["message"]["kind"].as_str())
        .unwrap_or("");
    matches!(
        kind,
        "user" | "user_message" | "user_prompt" | "prompt" | "UserMessage"
    )
}

pub(super) fn generate_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}
