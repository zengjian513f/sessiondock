use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::transport::Session;
use super::{Bridge, CachedModels};

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
                    // Kiro advertises a model's effort levels only inside its
                    // per-model `_meta.kiro.effortLevels`; `efforts`/
                    // `supportedReasoningEfforts` are absent. Fall back to that
                    // schema before the config-wide effort list.
                    let mut item_efforts: Vec<String> = item["efforts"]
                        .as_array()
                        .or_else(|| item["supportedReasoningEfforts"].as_array())
                        .or_else(|| item["_meta"]["kiro"]["effortLevels"].as_array())
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
                    let has_effort = item["_meta"]["kiro"]["hasEffort"].as_bool();
                    if has_effort == Some(false) {
                        item_efforts.clear();
                    }
                    // A model may carry its own default effort (Kiro's
                    // `_meta.kiro.defaultEffortLevel`); prefer it over the
                    // config-wide effort currentValue for this row.
                    let item_default_effort = item["_meta"]["kiro"]["defaultEffortLevel"]
                        .as_str()
                        .map(str::to_string)
                        .filter(|d| item_efforts.contains(d))
                        .or_else(|| default_effort.clone());
                    let mut row = json!({
                        "hasEffort": has_effort,
                        "id": mid,
                        "name": name,
                        "displayName": name,
                        "efforts": item_efforts,
                        "supportedReasoningEfforts": item_efforts,
                    });
                    if let Some(desc) = item["description"].as_str() {
                        row["description"] = json!(desc);
                    }
                    if let Some(de) = item_default_effort
                        .as_ref()
                        .filter(|_| !item_efforts.is_empty())
                    {
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
                let item_efforts = if m["_meta"]["kiro"]["hasEffort"] == false {
                    Vec::new()
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

fn ensure_model_in_list(list: &mut Vec<Value>, model_id: Option<&str>) {
    let Some(mid) = model_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    let exists = list
        .iter()
        .any(|item| item["id"].as_str() == Some(mid) || item["name"].as_str() == Some(mid));
    if !exists {
        let efforts: Vec<Value> = Vec::new();
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
        // Only an explicit CLI resume identity is evidence of external ownership.
        // Kiro's private lock files and storage layout are not an integration API.
        let tree = Arc::new(crate::runtime::procscan::ProcTree::open(
            self.proc_root.clone(),
        ));
        let roots = crate::runtime::procscan::SessionRoots::default();
        let scan = crate::runtime::procscan::scan(tree.clone(), None, &roots);
        let is_ours = |pid: i64| -> bool {
            let pid = pid.unsigned_abs() as u32;
            own_pid.is_some_and(|own| pid == own || tree.cli_ancestor(pid) == Some(own))
        };
        let sid_lower = format!("kiro:{}", sid.to_ascii_lowercase());
        if scan
            .sids
            .get(&sid_lower)
            .is_some_and(|pids| pids.iter().any(|pid| !is_ours(*pid)))
        {
            return Err(
                "kiro_sdk_session_busy: 该 Kiro 会话正在 CLI 终端中运行，请先停止 CLI 终端再启动 SDK 模式。"
                    .into(),
            );
        }
        Ok(())
    }

    pub(super) fn setting_params(
        session: &Arc<Session>,
        kind: &str,
        val: &str,
    ) -> Result<Value, String> {
        let sid = session.session_id.lock().unwrap().clone();
        if sid.is_empty() {
            return Err("session_id not initialized".into());
        }
        let (config_id, wire_value) = {
            let state = session.state.lock().unwrap();
            if matches!(kind, "effort" | "effortLevel") && !state.effort_supported() {
                return Err(
                    "kiro_sdk_setting_unavailable: 当前 Kiro 模型未广告 effort 配置".into(),
                );
            }
            let effort_option = matches!(kind, "effort" | "effortLevel")
                && state.config_options.iter().any(|option| {
                    let id = option["id"]
                        .as_str()
                        .or_else(|| option["configId"].as_str())
                        .unwrap_or("");
                    let category = option["category"].as_str().unwrap_or("");
                    ["effortLevel", "effort", "reasoningEffort"]
                        .iter()
                        .any(|name| id.eq_ignore_ascii_case(name))
                        || ["thought_level", "effort"]
                            .iter()
                            .any(|name| category.eq_ignore_ascii_case(name))
                });
            // Schema 2: Kiro embeds effort per model (`_meta.kiro.effortLevels`)
            // without a standalone config option. `effort_supported()` already
            // confirmed the selected model advertises levels; set it through the
            // native `effortLevel` config id, mirroring the create-time
            // `_meta.kiro.effortLevel` the CLI accepts.
            if matches!(kind, "effort" | "effortLevel") && !effort_option {
                return Ok(json!({"sessionId": sid, "configId": "effortLevel", "value": val}));
            }
            let option = state
                .config_options
                .iter()
                .find(|option| {
                    let id = option["id"]
                        .as_str()
                        .or_else(|| option["configId"].as_str())
                        .unwrap_or("");
                    let category = option["category"].as_str().unwrap_or("");
                    id.eq_ignore_ascii_case(kind)
                        || match kind {
                            "model" => {
                                id.eq_ignore_ascii_case("modelId")
                                    || category.eq_ignore_ascii_case("model")
                            }
                            "effort" | "effortLevel" => {
                                ["effortLevel", "effort", "reasoningEffort"]
                                    .iter()
                                    .any(|name| id.eq_ignore_ascii_case(name))
                                    || ["thought_level", "effort"]
                                        .iter()
                                        .any(|name| category.eq_ignore_ascii_case(name))
                            }
                            "mode" => category.eq_ignore_ascii_case("mode"),
                            "permission_mode" | "autopilot" => {
                                ["autopilot", "approval", "permissionMode"]
                                    .iter()
                                    .any(|name| id.eq_ignore_ascii_case(name))
                                    || ["autopilot", "approval"]
                                        .iter()
                                        .any(|name| category.eq_ignore_ascii_case(name))
                            }
                            _ => false,
                        }
                })
                .ok_or_else(|| {
                    format!("kiro_sdk_setting_unavailable: 当前 Kiro 未广告 {kind} 配置")
                })?;
            let id = option["id"]
                .as_str()
                .or_else(|| option["configId"].as_str())
                .ok_or("Kiro 配置缺少 ID")?;
            let wire = if matches!(kind, "permission_mode" | "autopilot") {
                let enabled = matches!(
                    val.to_ascii_lowercase().as_str(),
                    "true"
                        | "1"
                        | "on"
                        | "enabled"
                        | "autopilot"
                        | "auto"
                        | "always"
                        | "bypasspermissions"
                );
                let accepted: &[&str] = if enabled {
                    &["on", "true", "autopilot"]
                } else {
                    &["off", "false", "default"]
                };
                option["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|option| option["value"].as_str())
                    .find(|value| accepted.contains(value))
                    .ok_or("kiro_sdk_setting_unavailable: Kiro 未广告对应的自动批准选项")?
                    .to_owned()
            } else {
                val.to_owned()
            };
            (id.to_owned(), wire)
        };
        Ok(json!({"sessionId":sid,"configId":config_id,"value":wire_value}))
    }

    pub(super) async fn apply_setting(
        &self,
        session: &Arc<Session>,
        kind: &str,
        val: &str,
    ) -> Result<(), String> {
        let body = session
            .request(
                "session/set_config_option",
                Self::setting_params(session, kind, val)?,
            )
            .await?;
        let options = body["configOptions"]
            .as_array()
            .ok_or("kiro_sdk_setting_unconfirmed: Kiro 未返回更新后的配置，设置结果尚未确认")?;
        let mut state = session.state.lock().unwrap();
        state.apply_config_options(options);
        state.publish();
        Ok(())
    }

    pub(super) async fn models_action(
        &self,
        uid: &str,
        cwd: &str,
        target: &Value,
        refresh: bool,
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
                    if st.effort_supported() {
                        st.effort.clone().or(extracted_effort)
                    } else {
                        None
                    },
                    st.config_options.clone(),
                )
            };

            if cur_model.is_none() {
                let sid = self.resolve_sid(uid, target, &Value::Null);
                if let Some(metadata) = self.catalog.resolve_sid(&sid).map(|entry| entry.metadata) {
                    cur_model = metadata["_meta"]["kiro"]["modelId"]
                        .as_str()
                        .or_else(|| metadata["model"].as_str())
                        .or_else(|| metadata["currentModelId"].as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                    if cur_effort.is_none() {
                        cur_effort = metadata["_meta"]["kiro"]["effortLevel"]
                            .as_str()
                            .or_else(|| metadata["effort"].as_str())
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
                let (catalog_list, default_m) = self.catalog_or_probe_models(cwd, refresh).await;
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

        // 2. Use confirmed official catalog metadata when no SDK process is running.
        let (mut list, default_model) = self.catalog_or_probe_models(cwd, refresh).await;
        if !uid.is_empty() {
            let sid = self.resolve_sid(uid, target, &Value::Null);
            let mut catalog_model = None;
            let mut catalog_effort = None;
            if let Some(metadata) = self.catalog.resolve_sid(&sid).map(|entry| entry.metadata) {
                catalog_model = metadata["_meta"]["kiro"]["modelId"]
                    .as_str()
                    .or_else(|| metadata["model"].as_str())
                    .or_else(|| metadata["currentModelId"].as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                catalog_effort = metadata["_meta"]["kiro"]["effortLevel"]
                    .as_str()
                    .or_else(|| metadata["effort"].as_str())
                    .or_else(|| metadata["reasoningEffort"].as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                if let Some(arr) = metadata.get("configOptions").and_then(|v| v.as_array()) {
                    let (d_list, dm, de) = extract_models_from_config_options(arr, &Value::Null);
                    if !d_list.is_empty() {
                        list = d_list;
                    }
                    if catalog_model.is_none() {
                        catalog_model = dm;
                    }
                    if catalog_effort.is_none() {
                        catalog_effort = de;
                    }
                }
            }
            if catalog_model.is_none() {
                catalog_model = observed_from_target;
            }
            let model_observed = catalog_model.is_some();
            let chosen_model = catalog_model.or(default_model);
            ensure_model_in_list(&mut list, chosen_model.as_deref());
            return Ok(json!({
                "models": list,
                "current_model": chosen_model,
                "model": chosen_model,
                "model_observed": model_observed,
                "effort": catalog_effort,
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

    pub(super) async fn catalog_or_probe_models(
        &self,
        _cwd: &str,
        refresh: bool,
    ) -> (Vec<Value>, Option<String>) {
        // A running session's live config_options are the authoritative, always
        // fresh catalog and carry per-model effort metadata the CLI's
        // `--list-models` never reports. Prefer them even under `refresh`
        // (which only exists to bypass the time-based cache below): this gives
        // the new-session/bug-report picker real effort choices and skips the
        // multi-second CLI probe whenever a session of this source is live.
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

        if !refresh {
            if let Some(cached) = self.models_cache.lock().unwrap().as_ref()
                && cached.at.elapsed() < Duration::from_secs(600)
                && !cached.models.is_empty()
            {
                return (cached.models.clone(), cached.default_model.clone());
            }
        }

        // The CLI may wait for its account catalog over the network. Keep it
        // off Tokio workers so opening/replaying sessions and SSE keep progressing.
        let profile = self.profile.clone();
        let cat = tokio::task::spawn_blocking(move || crate::lifecycle::models::catalog(&profile))
            .await
            .unwrap_or_default();
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

        // CLI catalog failure must not create a native session merely to populate
        // a picker. Keep any previously confirmed catalog; otherwise show none.
        if let Some(cached) = self.models_cache.lock().unwrap().as_ref() {
            return (cached.models.clone(), cached.default_model.clone());
        }
        (Vec::new(), None)
    }

    pub(super) async fn respond(&self, uid: &str, payload: &Value) -> Result<Value, String> {
        let lock = self.lock_for(uid);
        let _guard = lock.lock().await;
        let session = self.get_session(uid)?;
        let key = payload["key"]
            .as_str()
            .ok_or("missing request key")?
            .to_string();
        let result = payload.get("result").cloned().unwrap_or(Value::Null);
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
                    .ok_or("Kiro permission response requires a native optionId")?;
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
        session.respond_pending(&key, rpc_res).await?;
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
        _cwd: &str,
        _target: &Value,
    ) -> Result<Value, String> {
        let session = self.get_session(uid)?;
        if !session
            .state
            .lock()
            .unwrap()
            .supports_method("_kiro/session/compact")
        {
            return Err("kiro_sdk_compact_unavailable: 当前 Kiro 未广告压缩能力".into());
        }
        let sid = session.session_id.lock().unwrap().clone();
        {
            let mut st = session.state.lock().unwrap();
            st.compact = Some(json!({ "state": "compacting" }));
            st.status = Some("compacting".into());
            st.publish();
        }
        let res = session
            .request("_kiro/session/compact", json!({"sessionId": sid}))
            .await;
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
        _uid: &str,
        _target: &Value,
        _payload: &Value,
    ) -> Result<Value, String> {
        Err("kiro_sdk_rename_unavailable: 当前 Kiro 未提供已确认的官方重命名接口".into())
    }

    pub(super) async fn fork_session(
        &self,
        uid: &str,
        cwd: &str,
        _target: &Value,
        payload: &Value,
    ) -> Result<Value, String> {
        if ["before", "nth_user", "turn_index", "prompt_index"]
            .iter()
            .any(|key| payload.get(*key).is_some_and(|value| !value.is_null()))
        {
            return Err(
                "kiro_sdk_fork_position_unavailable: Kiro 按整轮分支，请使用原生 messageId".into(),
            );
        }
        let source = self.catalog.resolve_uid(uid)?;
        self.check_conflict(&source.sid, None)?;
        let cwd = if cwd.is_empty() {
            source.cwd.as_str()
        } else {
            cwd
        };
        let request_id = payload["request_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or("Kiro 分叉请求缺少稳定 request_id")?;
        let mut params = json!({"sessionId":source.sid, "cwd":cwd, "mcpServers":[]});
        if let Some(message_id) = payload.get("messageId").filter(|value| !value.is_null()) {
            let message_id = message_id
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("Kiro 分支需要非空原生 messageId")?;
            // Kiro V3 rewind retains the selected turn in a new session.
            // The extension lives under _meta, not the standard fork fields.
            params["_meta"] = json!({"kiro":{"messageId":message_id,"createdReason":"rewind"}});
        }
        let result = self
            .mutate_once(
                &format!("fork:{}:{request_id}", source.uid),
                "session/fork",
                cwd,
                params,
            )
            .await?;
        let sid = result["sessionId"]
            .as_str()
            .filter(|sid| !sid.is_empty())
            .ok_or("kiro_sdk_fork_failed: 原生分叉未返回会话 ID；请刷新目录核对，不要重复分叉")?;
        if sid == source.sid {
            return Err("kiro_sdk_fork_failed: 原生分叉返回了源会话 ID".into());
        }
        let mut metadata = result.clone();
        metadata["cwd"] = json!(cwd);
        let entry = self.catalog.remember_native(sid, &metadata, None)?;
        Ok(
            json!({"uid":entry.uid,"session_id":entry.sid,"cwd":entry.cwd,
            "session":{"uid":entry.uid,"sid":entry.sid,"session_id":entry.sid,
            "source":"kiro","cwd":entry.cwd,"path":entry.path,"sdk":true}}),
        )
    }

    pub(super) async fn delete_session(&self, uid: &str) -> Result<Value, String> {
        let entry = self.catalog.resolve_uid(uid)?;
        if self
            .loaded_uids()
            .iter()
            .any(|loaded| loaded == uid || loaded == &entry.uid)
        {
            return Err("kiro_sdk_session_busy: 请先停止 SDK 会话再删除".into());
        }
        self.check_conflict(&entry.sid, None)?;
        self.mutate_once(
            &format!("delete:{}", entry.uid),
            "session/delete",
            &entry.cwd,
            json!({"sessionId":entry.sid}),
        )
        .await?;
        self.catalog.mark_deleted(uid)?;
        Ok(json!({"uid":uid,"discarded":true,"native_deleted":true}))
    }
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
