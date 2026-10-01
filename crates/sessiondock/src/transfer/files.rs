//! Native file bundles for Claude and Grok. Only structured native identity
//! fields are rewritten; message text, source files and arbitrary attachments
//! are never treated as identity strings.
use super::{TransferError, codex, group::Group};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct File {
    pub source: PathBuf,
    pub relative: PathBuf,
    pub target: PathBuf,
    pub provider: String,
    pub owner: String,
    pub bytes: u64,
    pub sha256: String,
    pub format: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub group: Group,
    pub new_ids: bool,
    /// Keys include provider to keep equal IDs in different CLIs separate.
    pub sessions: BTreeMap<String, String>,
    pub records: BTreeMap<String, String>,
    #[serde(default)]
    pub tool_names: BTreeMap<String, String>,
    #[serde(default)]
    pub owner_tools: BTreeMap<String, BTreeMap<String, String>>,
    pub roots: BTreeMap<String, PathBuf>,
    pub files: Vec<File>,
}
fn digest(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn key(source: &str, id: &str) -> String {
    format!("{source}:{id}")
}
fn mint(
    source: &str,
    id: &str,
    map: &mut BTreeMap<String, String>,
    fresh: bool,
) -> Result<(), TransferError> {
    if !id.is_empty() && !map.contains_key(&key(source, id)) {
        map.insert(
            key(source, id),
            if fresh {
                let next = codex::uuid()?;
                if id.starts_with("ag1.") {
                    format!("ag1.{}", next.replace('-', ""))
                } else {
                    next
                }
            } else {
                id.into()
            },
        );
    }
    Ok(())
}
fn rewrite_scalar(value: &mut Value, source: &str, map: &BTreeMap<String, String>) {
    if let Some(id) = value.as_str()
        && let Some(new) = map.get(&key(source, id))
    {
        *value = Value::String(new.clone());
    }
}
fn field(value: &mut Value, name: &str, source: &str, map: &BTreeMap<String, String>) {
    if let Some(value) = value.get_mut(name) {
        rewrite_scalar(value, source, map);
    }
}
/// Enumerate only native structural fields, never arbitrary tool arguments.
fn record_ids(row: &mut Value, source: &str, visit: &mut impl FnMut(&mut Value)) {
    for name in [
        "uuid",
        "parentUuid",
        "leafUuid",
        "logicalParentUuid",
        "parentLastUuid",
        "promptId",
        "messageId",
        "interruptedMessageId",
        "toolUseId",
        "parentToolUseId",
        "tool_use_id",
        "parent_tool_use_id",
        "toolCallId",
        "tool_call_id",
    ] {
        if let Some(v) = row.get_mut(name) {
            visit(v);
        }
    }
    if let Some(snapshot) = row.get_mut("snapshot")
        && let Some(id) = snapshot.get_mut("messageId")
    {
        visit(id);
    }
    if let Some(message) = row.get_mut("message") {
        // Claude's message.id is the API response identity, used by resume's
        // diagnostics.previous_message_id. It belongs to the server; the local
        // transcript identity is row.uuid and is remapped separately.
        if source != "claude"
            && let Some(id) = message.get_mut("id")
        {
            visit(id);
        }
        content_ids(message.get_mut("content"), visit);
    }
    content_ids(row.get_mut("content"), visit);
    // Grok/OpenAI chat messages store calls separately from content blocks.
    if source == "grok" {
        for name in [
            "id",
            "eventId",
            "chunkId",
            "prompt_id",
            "parent_prompt_id",
            "checkpoint_id",
        ] {
            if let Some(value) = row.get_mut(name) {
                visit(value);
            }
        }
        if let Some(calls) = row.get_mut("tool_calls").and_then(Value::as_array_mut) {
            for call in calls {
                if let Some(id) = call.get_mut("id") {
                    visit(id);
                }
            }
        }
        for item in row
            .get_mut("compacted_history")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            record_ids(item, source, visit);
        }
        if let Some(update) = row.get_mut("update") {
            record_ids(update, source, visit);
        }
        for wrapper in ["params", "_meta", "updateParams"] {
            if let Some(update) = row.get_mut(wrapper) {
                record_ids(update, source, visit);
            }
        }
    }
}
// Only the native checkpoint pointer is a path identity. Summary text and
// reread_file_paths describe the workspace and remain unchanged.
fn rewrite_checkpoint_paths(row: &mut Value, records: &BTreeMap<String, String>) {
    if let Some(pointer) = row.get_mut("checkpoint_file")
        && let Some(id) = pointer
            .as_str()
            .and_then(|s| s.strip_prefix("compaction_checkpoints/"))
            .and_then(|s| s.strip_suffix(".json"))
        && let Some(mapped) = records.get(&key("grok", id))
    {
        *pointer = format!("compaction_checkpoints/{mapped}.json").into();
    }
    for wrapper in ["params", "update", "_meta", "updateParams"] {
        if let Some(child) = row.get_mut(wrapper) {
            rewrite_checkpoint_paths(child, records);
        }
    }
}
fn content_ids(content: Option<&mut Value>, visit: &mut impl FnMut(&mut Value)) {
    if let Some(items) = content.and_then(Value::as_array_mut) {
        for item in items {
            match item["type"].as_str() {
                Some("tool_use") => {
                    if let Some(id) = item.get_mut("id") {
                        visit(id);
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = item.get_mut("tool_use_id") {
                        visit(id);
                    }
                }
                _ => {}
            }
        }
    }
}
fn session_ids(row: &mut Value, source: &str, ids: &BTreeMap<String, String>) {
    for name in [
        "sessionId",
        "session_id",
        "parentSessionId",
        "parent_session_id",
        "forkedFromSessionId",
        "continuedInSessionId",
        "agentId",
        "parentAgentId",
        "agent_id",
        "subagent_id",
        "child_session_id",
    ] {
        field(row, name, source, ids);
    }
    if source == "grok" {
        for item in row
            .get_mut("compacted_history")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            session_ids(item, source, ids);
        }
        if let Some(info) = row.get_mut("info") {
            field(info, "id", source, ids);
        }
    }
    for wrapper in ["params", "update", "toolUseResult"] {
        if let Some(child) = row.get_mut(wrapper) {
            session_ids(child, source, ids);
        }
    }
}
pub(super) fn collect_tools(row: &Value, source: &str, names: &mut BTreeMap<String, String>) {
    if source == "grok" {
        for item in row["compacted_history"].as_array().into_iter().flatten() {
            collect_tools(item, source, names);
        }
    }
    for call in row["tool_calls"].as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (call["id"].as_str(), call["name"].as_str()) {
            names.insert(key(source, id), name.into());
        }
    }
    for block in row["message"]["content"].as_array().into_iter().flatten() {
        if block["type"] == "tool_use"
            && let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str())
        {
            names.insert(key(source, id), name.into());
        }
    }
    if row["sessionUpdate"] == "tool_call"
        && let (Some(id), Some(name)) = (row["toolCallId"].as_str(), row["title"].as_str())
    {
        names.insert(key(source, id), name.into());
    }
    for wrapper in ["params", "update"] {
        if row[wrapper].is_object() {
            collect_tools(&row[wrapper], source, names);
        }
    }
}
pub(super) fn agent_tool(name: &str) -> bool {
    matches!(
        name,
        "Agent"
            | "SendMessage"
            | "Task"
            | "TaskOutput"
            | "spawn_subagent"
            | "send_subagent_message"
            | "get_command_or_subagent_output"
            | "kill_subagent"
    )
}
fn agent_args(value: &mut Value, source: &str, ids: &BTreeMap<String, String>) {
    for name in ["resume", "resume_from", "subagent_id", "task_id"] {
        field(value, name, source, ids);
    }
    if let Some(list) = value.get_mut("task_ids").and_then(Value::as_array_mut) {
        for id in list {
            rewrite_scalar(id, source, ids);
        }
    }
}
fn agent_text(value: &mut Value, source: &str, ids: &BTreeMap<String, String>) {
    if let Some(text) = value.as_str() {
        if source == "grok"
            && let Ok(mut parsed) = serde_json::from_str::<Value>(text)
            && (parsed.is_object() || parsed.is_array())
        {
            let original = parsed.clone();
            agent_text(&mut parsed, source, ids);
            if parsed != original {
                *value = Value::String(parsed.to_string());
            }
            return;
        }
        let mut result = text.to_owned();
        for (id, new) in ids {
            if let Some(old) = id.strip_prefix(&format!("{source}:")) {
                // Native result envelopes only. Literal IDs in prompts and agent
                // answer bodies stay untouched.
                for (prefix, suffix) in [
                    ("subagent_id: ", "\n"),
                    ("agentId: ", " "),
                    ("=== Task ", " ==="),
                    ("<subagent_meta>id=", ","),
                    ("resume_from=\"", "\""),
                    ("task_ids=[\"", "\"]"),
                ] {
                    result = result.replace(
                        &format!("{prefix}{old}{suffix}"),
                        &format!("{prefix}{new}{suffix}"),
                    );
                }
            }
        }
        *value = Value::String(result);
    } else if let Some(items) = value.as_array_mut() {
        for item in items {
            if item["type"] == "text"
                && let Some(text) = item.get_mut("text")
            {
                agent_text(text, source, ids);
            }
        }
    } else if let Some(object) = value.as_object_mut() {
        for name in ["subagent_id", "child_session_id", "task_id", "agentId"] {
            if let Some(value) = object.get_mut(name) {
                rewrite_scalar(value, source, ids);
            }
        }
        for name in ["text", "output", "Result"] {
            if let Some(value) = object.get_mut(name) {
                agent_text(value, source, ids);
            }
        }
    }
}
fn rewrite_tools(
    row: &mut Value,
    source: &str,
    ids: &BTreeMap<String, String>,
    names: &BTreeMap<String, String>,
) -> Result<(), TransferError> {
    if source == "grok" {
        for item in row
            .get_mut("compacted_history")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            rewrite_tools(item, source, ids, names)?;
        }
    }
    let mut send_message_result = false;
    if let Some(calls) = row.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for call in calls {
            if call["name"].as_str().is_some_and(agent_tool) {
                if let Some(raw) = call["arguments"].as_str() {
                    let mut args: Value = serde_json::from_str(raw)?;
                    agent_args(&mut args, source, ids);
                    call["arguments"] = serde_json::to_string(&args)?.into();
                } else if let Some(args) = call.get_mut("arguments") {
                    agent_args(args, source, ids);
                }
            }
        }
    }
    let name = row["tool_call_id"]
        .as_str()
        .or_else(|| row["toolCallId"].as_str())
        .and_then(|id| names.get(&key(source, id)));
    if name.is_some_and(|s| agent_tool(s)) {
        if let Some(args) = row.get_mut("rawInput") {
            agent_args(args, source, ids);
        }
        for field in ["content", "rawOutput"] {
            if let Some(value) = row.get_mut(field) {
                agent_text(value, source, ids);
            }
        }
    }
    if let Some(items) = row
        .get_mut("message")
        .and_then(|m| m.get_mut("content"))
        .and_then(Value::as_array_mut)
    {
        for item in items {
            // SendMessage resumes an existing Claude subagent. Its destination
            // is an identity only in this tool's structured input; ordinary
            // message text and other tools' `to` fields remain unchanged.
            if source == "claude" && item["type"] == "tool_use" && item["name"] == "SendMessage" {
                field(&mut item["input"], "to", source, ids);
            }
            if item["type"] == "tool_use" && item["name"].as_str().is_some_and(agent_tool) {
                agent_args(&mut item["input"], source, ids);
            }
            if item["type"] == "tool_result"
                && item["tool_use_id"]
                    .as_str()
                    .and_then(|id| names.get(&key(source, id)))
                    .is_some_and(|s| agent_tool(s))
            {
                if source == "claude"
                    && item["tool_use_id"]
                        .as_str()
                        .and_then(|id| names.get(&key(source, id)))
                        .is_some_and(|name| name == "SendMessage")
                {
                    rewrite_send_message_result(&mut item["content"], ids);
                    send_message_result = true;
                } else {
                    agent_text(&mut item["content"], source, ids);
                }
            }
        }
    }
    if send_message_result && let Some(result) = row.get_mut("toolUseResult") {
        rewrite_send_message_result(result, ids);
    }
    for wrapper in ["params", "update"] {
        if let Some(child) = row.get_mut(wrapper) {
            rewrite_tools(child, source, ids, names)?;
        }
    }
    Ok(())
}

fn rewrite_send_message_result(value: &mut Value, ids: &BTreeMap<String, String>) {
    if let Some(text) = value.as_str() {
        if let Ok(mut result) = serde_json::from_str::<Value>(text)
            && result.is_object()
            && result.get("resumedAgentId").is_some()
        {
            rewrite_send_message_result(&mut result, ids);
            *value = Value::String(result.to_string());
        }
    } else if let Some(items) = value.as_array_mut() {
        for item in items {
            if item["type"] == "text" {
                rewrite_send_message_result(&mut item["text"], ids);
            }
        }
    } else if let Some(old) = value["resumedAgentId"].as_str().map(str::to_owned)
        && let Some(new) = ids.get(&key("claude", &old))
    {
        if value["message"] == format!("Resuming agent {}", old.chars().take(7).collect::<String>())
        {
            value["message"] =
                format!("Resuming agent {}", new.chars().take(7).collect::<String>()).into();
        }
        field(value, "resumedAgentId", "claude", ids);
        if let Some(pin) = value.get_mut("pin") {
            field(pin, "id", "claude", ids);
            field(pin, "name", "claude", ids);
        }
    }
}
fn parse(raw: &[u8], format: &str) -> Result<Vec<Value>, TransferError> {
    if format == "json" {
        return Ok(vec![serde_json::from_slice(raw)?]);
    }
    raw.split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .map(|line| serde_json::from_slice(line).map_err(Into::into))
        .collect()
}
fn native_format(source: &str, path: &Path) -> &'static str {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if source == "claude" {
        if name.ends_with(".jsonl")
            && (path
                .parent()
                .is_some_and(|p| p.file_name().is_some_and(|s| s == "subagents"))
                || name.strip_suffix(".jsonl").is_some_and(|s| s.len() == 36))
        {
            return "jsonl";
        }
        if name.starts_with("agent-") && name.ends_with(".meta.json") {
            return "json";
        }
    } else {
        if path
            .parent()
            .is_some_and(|p| p.file_name().is_some_and(|s| s == "compaction_checkpoints"))
            && path.extension().is_some_and(|s| s == "json")
        {
            return "json";
        }
        if matches!(
            name,
            "summary.json" | "meta.json" | "signals.json" | "rewind_state.json"
        ) {
            return "json";
        }
        if matches!(
            name,
            "chat_history.jsonl" | "updates.jsonl" | "events.jsonl" | "rewind_points.jsonl"
        ) {
            return "jsonl";
        }
    }
    "raw"
}
fn walk(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), TransferError> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_symlink() {
        // Preserve the link itself. Targets are validated as external dependencies
        // by the destination preflight, never traversed while bundling a session.
        out.push(path.into());
    } else if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            walk(&entry?.path(), out)?;
        }
    } else {
        out.push(path.into());
    }
    Ok(())
}
fn mapped_path(path: &Path, source: &str, ids: &BTreeMap<String, String>) -> PathBuf {
    path.components()
        .map(|c| {
            let text = c.as_os_str().to_string_lossy();
            let mapped = ids.get(&key(source, &text)).cloned().or_else(|| {
                for (prefix, suffix) in [
                    ("", ".jsonl"),
                    ("agent-", ".jsonl"),
                    ("agent-", ".meta.json"),
                ] {
                    if let Some(id) = text
                        .strip_prefix(prefix)
                        .and_then(|s| s.strip_suffix(suffix))
                        && let Some(new) = ids.get(&key(source, id))
                    {
                        return Some(format!("{prefix}{new}{suffix}"));
                    }
                }
                None
            });
            mapped
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(c.as_os_str()))
        })
        .collect()
}
impl Plan {
    pub fn build(group: Group, new_ids: bool) -> Result<Self, TransferError> {
        if !group.blockers.is_empty() {
            return Err(TransferError::new(
                "move_group_incomplete",
                group
                    .blockers
                    .iter()
                    .map(|b| b.message.as_str())
                    .collect::<Vec<_>>()
                    .join("；"),
            ));
        }
        let mut plan = Self {
            group,
            new_ids,
            sessions: BTreeMap::new(),
            records: BTreeMap::new(),
            tool_names: BTreeMap::new(),
            owner_tools: BTreeMap::new(),
            roots: BTreeMap::new(),
            files: Vec::new(),
        };
        let mut paths = BTreeMap::<PathBuf, (String, String)>::new();
        for member in &plan.group.members {
            if !matches!(member.source.as_str(), "claude" | "grok") {
                return Err(TransferError::new(
                    "move_group_unsupported",
                    "文件适配器仅处理 Claude 和 Grok",
                ));
            }
            mint(&member.source, &member.sid, &mut plan.sessions, new_ids)?;
            if member.agent && member.source == "claude" && new_ids {
                // Claude uses short hexadecimal agent IDs in sidecar filenames.
                let id = codex::uuid()?.replace('-', "");
                plan.sessions
                    .insert(key("claude", &member.sid), format!("a{}", &id[..16]));
            }
            let home = if member.source == "claude"
                && member.root.file_name().is_some_and(|s| s == "projects")
            {
                member.root.parent().unwrap().to_owned()
            } else {
                member.root.clone()
            };
            plan.roots.insert(member.source.clone(), home);
            let mut owned = Vec::new();
            if member.source == "grok" {
                let summary = member.path.parent().unwrap().join("summary.json");
                if summary.is_file() {
                    let row: Value = serde_json::from_slice(&fs::read(summary)?)?;
                    if let Some(agent) = row["agent_id"].as_str() {
                        mint("grok", agent, &mut plan.sessions, new_ids)?;
                    }
                }
                walk(member.path.parent().unwrap(), &mut owned)?;
            } else {
                walk(&member.path, &mut owned)?;
                if !member.agent {
                    let side = member.path.with_extension("");
                    if side.exists() {
                        walk(&side, &mut owned)?;
                    }
                    let history = plan.roots["claude"].join("file-history").join(&member.sid);
                    if history.exists() {
                        walk(&history, &mut owned)?;
                    }
                } else {
                    let meta = member.path.with_extension("meta.json");
                    if meta.exists() {
                        walk(&meta, &mut owned)?;
                    }
                }
            }
            for path in owned {
                paths
                    .entry(path)
                    .or_insert((member.source.clone(), member.uid.clone()));
            }
        }
        for (path, (source, owner)) in paths {
            let relative = path
                .strip_prefix(&plan.roots[&source])
                .map_err(|_| TransferError::new("move_path", "会话文件不在原生根目录内"))?
                .to_owned();
            if !relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
            {
                return Err(TransferError::new("move_path", "无效的会话相对路径"));
            }
            let link = fs::symlink_metadata(&path)?.is_symlink();
            let raw = if link {
                fs::read_link(&path)?.to_string_lossy().as_bytes().to_vec()
            } else {
                fs::read(&path)?
            };
            let format = if link {
                "symlink"
            } else if plan.group.members.iter().any(|m| m.path == path) {
                "jsonl"
            } else {
                native_format(&source, &path)
            };
            if matches!(format, "json" | "jsonl") {
                for mut row in parse(&raw, format)? {
                    if source == "grok"
                        && path.file_name().is_some_and(|s| s == "summary.json")
                        && let Some(agent) = row["agent_id"].as_str()
                    {
                        mint(&source, agent, &mut plan.sessions, new_ids)?;
                    }
                    collect_tools(&row, &source, &mut plan.tool_names);
                    if source == "grok" {
                        collect_tools(
                            &row,
                            &source,
                            plan.owner_tools.entry(owner.clone()).or_default(),
                        );
                    }
                    let mut error = None;
                    record_ids(&mut row, &source, &mut |value| {
                        if let Some(id) = value.as_str()
                            && let Err(e) = mint(&source, id, &mut plan.records, new_ids)
                        {
                            error = Some(e);
                        }
                    });
                    if let Some(e) = error {
                        return Err(e);
                    }
                }
            }
            let target = mapped_path(&relative, &source, &plan.sessions);
            plan.files.push(File {
                source: path,
                relative,
                target,
                provider: source,
                owner,
                bytes: raw.len() as u64,
                sha256: digest(&raw),
                format: format.into(),
            });
        }
        // Checkpoint paths are session-relative and referenced by updates.jsonl.
        // All record IDs must be collected before assigning their filenames.
        for file in &mut plan.files {
            if file.provider == "grok"
                && file
                    .source
                    .parent()
                    .is_some_and(|p| p.file_name().is_some_and(|s| s == "compaction_checkpoints"))
                && let Some(id) = file
                    .source
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|id| plan.records.get(&key("grok", id)))
            {
                file.target.set_file_name(format!("{id}.json"));
            }
        }
        Ok(plan)
    }
    pub fn rewrite(&self, file: &File, raw: &[u8]) -> Result<Vec<u8>, TransferError> {
        if digest(raw) != file.sha256 {
            return Err(TransferError::new(
                "move_plan_stale",
                "原生文件在计划后发生变化",
            ));
        }
        if self.new_ids && file.format == "symlink" {
            let link = PathBuf::from(
                String::from_utf8(raw.to_vec())
                    .map_err(|e| TransferError::new("move_path", e.to_string()))?,
            );
            let absolute = if link.is_absolute() {
                link
            } else {
                file.source.parent().unwrap().join(link)
            };
            let mut normalized = PathBuf::new();
            for component in absolute.components() {
                match component {
                    Component::ParentDir => {
                        normalized.pop();
                    }
                    Component::CurDir => {}
                    _ => normalized.push(component.as_os_str()),
                }
            }
            // Only rewrite targets belonging to this bundle. External links
            // retain their original meaning, including relative spelling.
            if self
                .files
                .iter()
                .any(|f| f.provider == file.provider && f.source.starts_with(&normalized))
            {
                let root = &self.roots[&file.provider];
                if let Ok(relative) = normalized.strip_prefix(root) {
                    let mapped = root.join(mapped_path(relative, &file.provider, &self.sessions));
                    if mapped != normalized {
                        return Ok(mapped.to_string_lossy().as_bytes().to_vec());
                    }
                }
            }
            return Ok(raw.to_vec());
        }
        if !self.new_ids || !matches!(file.format.as_str(), "json" | "jsonl") {
            return Ok(raw.to_vec());
        }
        let source = &file.provider;
        let mut output = Vec::new();
        let rows = parse(raw, &file.format)?;
        let links = (source == "claude").then(|| super::group::claude_tools::links(&rows));
        let mut local_names = BTreeMap::new();
        if source == "claude" {
            for row in &rows {
                collect_tools(row, source, &mut local_names);
            }
        }
        let names = if source == "grok" {
            self.owner_tools.get(&file.owner).unwrap_or(&local_names)
        } else {
            &local_names
        };
        for mut row in rows {
            if let Some(links) = &links {
                super::group::claude_tools::resolve(&mut row, links);
            }
            session_ids(&mut row, source, &self.sessions);
            rewrite_tools(&mut row, source, &self.sessions, names)?;
            self.tool_paths(&mut row, source);
            if source == "grok" {
                rewrite_checkpoint_paths(&mut row, &self.records);
            }
            record_ids(&mut row, source, &mut |value| {
                rewrite_scalar(value, source, &self.records)
            });
            serde_json::to_writer(&mut output, &row)?;
            output.push(b'\n');
        }
        Ok(output)
    }
    fn tool_paths(&self, row: &mut Value, source: &str) {
        let paths = self
            .files
            .iter()
            .filter(|f| f.provider == source)
            .map(|f| {
                (
                    f.source.to_string_lossy().into_owned(),
                    self.roots[source]
                        .join(&f.target)
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        fn result(value: &mut Value, paths: &BTreeMap<String, String>) {
            if let Some(object) = value.as_object_mut() {
                for name in [
                    "filePath",
                    "file_path",
                    "outputFile",
                    "output_file",
                    "transcriptPath",
                ] {
                    if let Some(v) = object.get_mut(name)
                        && let Some(mapped) = v.as_str().and_then(|s| paths.get(s))
                    {
                        *v = mapped.clone().into();
                    }
                }
                for name in ["content", "text", "stdout", "stderr"] {
                    if let Some(v) = object.get_mut(name) {
                        result(v, paths);
                    }
                }
            } else if let Some(items) = value.as_array_mut() {
                for item in items {
                    result(item, paths);
                }
            } else if let Some(text) = value.as_str()
                && (text.contains("<persisted-output>")
                    || text.starts_with("Full output saved to:"))
            {
                let mut rewritten = text.to_owned();
                for (old, new) in paths {
                    // Match the native output pointer, not UUIDs or arbitrary
                    // paths in a user's message / tool's ordinary output.
                    for prefix in ["saved to: ", "Saved to: "] {
                        for suffix in ["\n", "\r", "</persisted-output>"] {
                            rewritten = rewritten.replace(
                                &format!("{prefix}{old}{suffix}"),
                                &format!("{prefix}{new}{suffix}"),
                            );
                        }
                        if rewritten.ends_with(&format!("{prefix}{old}")) {
                            rewritten.truncate(rewritten.len() - old.len());
                            rewritten.push_str(new);
                        }
                    }
                }
                *value = rewritten.into();
            }
        }
        if let Some(output) = row.get_mut("toolUseResult") {
            result(output, &paths);
        }
        if let Some(content) = row
            .get_mut("message")
            .and_then(|m| m.get_mut("content"))
            .and_then(Value::as_array_mut)
        {
            for item in content {
                if item["type"] == "tool_result" {
                    result(&mut item["content"], &paths);
                }
            }
        }
    }
    pub fn member_target(&self, member: &super::group::Member) -> Result<PathBuf, TransferError> {
        let root = &self.roots[&member.source];
        let relative = member
            .path
            .strip_prefix(root)
            .map_err(|_| TransferError::new("move_path", "会话不在原生根目录中"))?;
        let path = root.join(mapped_path(relative, &member.source, &self.sessions));
        Ok(if member.source == "grok" {
            path.parent().unwrap().to_owned()
        } else {
            path
        })
    }
    pub fn stage(&self, destination: &Path) -> Result<(), TransferError> {
        // Older journals did not persist owner-scoped tool names. Reconstruct
        // them once per staging operation, never once per output file.
        let mut prepared = self.clone();
        if self.new_ids && prepared.owner_tools.is_empty() {
            for file in self
                .files
                .iter()
                .filter(|f| f.provider == "grok" && matches!(f.format.as_str(), "json" | "jsonl"))
            {
                super::coordination::check()?;
                let raw = fs::read(&file.source)?;
                if digest(&raw) != file.sha256 {
                    return Err(TransferError::new(
                        "move_plan_stale",
                        "原生文件在计划后发生变化",
                    ));
                }
                for row in parse(&raw, &file.format)? {
                    collect_tools(
                        &row,
                        "grok",
                        prepared.owner_tools.entry(file.owner.clone()).or_default(),
                    );
                }
            }
        }
        if destination.exists() {
            return Err(TransferError::new("move_conflict", "暂存目录已存在"));
        }
        let parent = destination
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()?;
        for root in self.roots.values() {
            if parent.starts_with(root.canonicalize()?) {
                return Err(TransferError::new(
                    "move_path",
                    "暂存目录不能位于源会话根目录内",
                ));
            }
        }
        fs::create_dir(destination)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
        }
        for file in &self.files {
            let raw = if file.format == "symlink" {
                fs::read_link(&file.source)?
                    .to_string_lossy()
                    .as_bytes()
                    .to_vec()
            } else {
                fs::read(&file.source)?
            };
            if !matches!(file.provider.as_str(), "claude" | "grok")
                || !file
                    .target
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)))
            {
                return Err(TransferError::new("move_path", "无效的暂存目标路径"));
            }
            let target = destination.join(&file.provider).join(&file.target);
            fs::create_dir_all(target.parent().unwrap())?;
            super::coordination::check()?;
            let output = prepared.rewrite(file, &raw)?;
            if file.format == "symlink" {
                #[cfg(unix)]
                std::os::unix::fs::symlink(
                    String::from_utf8(output)
                        .map_err(|e| TransferError::new("move_path", e.to_string()))?,
                    target,
                )?;
                #[cfg(not(unix))]
                return Err(TransferError::new(
                    "move_platform",
                    "此平台不支持迁移符号链接",
                ));
            } else {
                fs::write(&target, output)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let executable = fs::metadata(&file.source)?.permissions().mode() & 0o111;
                    fs::set_permissions(&target, fs::Permissions::from_mode(0o600 | executable))?;
                }
            }
        }
        for file in &self.files {
            let raw = if file.format == "symlink" {
                fs::read_link(&file.source)?
                    .to_string_lossy()
                    .as_bytes()
                    .to_vec()
            } else {
                fs::read(&file.source)?
            };
            if digest(&raw) != file.sha256 {
                return Err(TransferError::new(
                    "move_plan_stale",
                    "暂存期间源文件发生变化",
                ));
            }
        }
        fs::write(
            destination.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "publishable":false,"plan":self,"required_checks":["native_references","native_resume","destination_preflight","publication_transaction"]
            }))?,
        )?;
        Ok(())
    }
}
