//! Compact native relationship summaries. Cache file metadata, never transcripts.
use super::{
    TransferError,
    environment::{self, Stamp},
    group::{claude_tools, grok_tools},
};
use crate::sessions::CandidateRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Links {
    pub required: BTreeSet<(String, String)>,
    pub optional: BTreeSet<String>,
    pub messages: BTreeSet<String>,
    pub alias: Option<String>,
    pub calls: BTreeMap<String, String>,
    pub outputs: BTreeMap<String, BTreeSet<String>>,
    pub errors: Vec<(String, String)>,
}
type Key = Vec<(PathBuf, Option<Stamp>)>;
#[derive(Default)]
pub(super) struct Cache {
    entries: Mutex<BTreeMap<PathBuf, (Key, Arc<Links>)>>,
    path: Option<PathBuf>,
    dirty: AtomicBool,
}
impl Cache {
    pub fn open(path: PathBuf) -> Self {
        let entries = fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Vec<(PathBuf, Key, Links)>>(&raw).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(path, key, links)| (path, (key, Arc::new(links))))
            .collect();
        Self {
            entries: Mutex::new(entries),
            path: Some(path),
            dirty: AtomicBool::new(false),
        }
    }
    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Ok(entries) = self.entries.lock() {
            if !self.dirty.load(Ordering::Relaxed) {
                return;
            }
            let rows: Vec<_> = entries
                .iter()
                .map(|(path, (key, links))| (path, key, links.as_ref()))
                .collect();
            // Disposable acceleration only: unreadable/stale summaries fall back
            // to native files; a failed cache write must not fail a transfer.
            if let Err(error) = super::service::persist(path, &rows) {
                eprintln!("transfer relationship cache: {}", error.message);
            } else {
                self.dirty.store(false, Ordering::Relaxed);
            }
        }
    }
    pub fn retain(&self, paths: &BTreeSet<PathBuf>) {
        if let Ok(mut cache) = self.entries.lock() {
            let previous = cache.len();
            cache.retain(|path, _| paths.contains(path));
            if previous != cache.len() {
                self.dirty.store(true, Ordering::Relaxed);
            }
        }
    }
    pub fn get(&self, e: &CandidateRef) -> Result<Arc<Links>, TransferError> {
        let before = match key(e) {
            Ok(key) => key,
            // Cache metadata is an optimization, not an extra input policy.
            Err(_) => return Ok(Arc::new(read(e))),
        };
        if let Some((_, links)) = self.entries.lock().ok().and_then(|cache| {
            cache
                .get(&e.data)
                .filter(|(key, _)| *key == before)
                .cloned()
        }) {
            return Ok(links);
        }
        let links = Arc::new(read(e));
        // A concurrent writer must never associate a partial read with its final stamp.
        if links.errors.is_empty() && key(e).ok().as_ref() == Some(&before) {
            if let Ok(mut cache) = self.entries.lock() {
                cache.insert(e.data.clone(), (before, links.clone()));
                self.dirty.store(true, Ordering::Relaxed);
            }
        }
        Ok(links)
    }
}
fn key(e: &CandidateRef) -> Result<Key, TransferError> {
    let mut paths = BTreeSet::from([e.data.clone()]);
    if e.source == "grok" {
        if let Some(summary) = &e.summary_path {
            paths.insert(summary.clone());
            let root = summary.parent().unwrap();
            paths.insert(root.join("updates.jsonl"));
            for (directory, nested) in [("compaction_checkpoints", false), ("subagents", true)] {
                let dir = root.join(directory);
                if dir.is_dir() {
                    for entry in fs::read_dir(dir)? {
                        let path = entry?.path();
                        if nested {
                            paths.insert(path.join("meta.json"));
                        } else if path.extension().is_some_and(|v| v == "json") {
                            paths.insert(path);
                        }
                    }
                }
            }
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let stamp = match fs::metadata(&path) {
                Ok(meta) => Some(environment::stamp(&meta)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            Ok((path, stamp))
        })
        .collect()
}
fn read(e: &CandidateRef) -> Links {
    let mut links = Links::default();
    if let Err(error) = extract(e, &mut links) {
        links.errors.push((error.code, error.message));
    }
    links
}

// Parse structure without allocating prompt text, images, reasoning, or shell
// output that cannot establish a relationship. RawValue still validates JSON;
// the existing provider adapters remain the authority for interpreting fields.
fn relationship_row(line: &[u8], source: &str) -> serde_json::Result<Value> {
    type Object<'a> = BTreeMap<String, &'a RawValue>;
    fn pick(raw: &Object<'_>, keys: &[&str]) -> serde_json::Result<Value> {
        let mut result = serde_json::Map::new();
        for key in keys {
            if let Some(value) = raw.get(*key) {
                result.insert((*key).into(), serde_json::from_str(value.get())?);
            }
        }
        Ok(Value::Object(result))
    }
    fn text(raw: &Object<'_>, key: &str) -> Option<String> {
        raw.get(key)
            .and_then(|v| serde_json::from_str(v.get()).ok())
    }
    let Ok(raw) = serde_json::from_slice::<Object<'_>>(line) else {
        return serde_json::from_slice(line);
    };
    if source == "codex" {
        let mut row = pick(&raw, &["type"])?;
        let Some(payload) = raw.get("payload") else {
            return Ok(row);
        };
        let Ok(p) = serde_json::from_str::<Object<'_>>(payload.get()) else {
            return Ok(row);
        };
        let keys: &[&str] = match row["type"].as_str() {
            Some("event_msg") => &[
                "type",
                "thread_id",
                "sender_thread_id",
                "receiver_thread_id",
                "new_thread_id",
                "agent_thread_id",
                "senderThreadId",
                "receiverThreadId",
                "agentThreadId",
                "receiver_thread_ids",
                "receiverThreadIds",
                "receiver_agents",
                "agent_statuses",
                "receiverAgents",
                "agentStatuses",
                "statuses",
                "agents_states",
                "agentsStates",
                "threadId",
                "goal",
                "item",
            ],
            Some("response_item") => match text(&p, "type").as_deref() {
                Some("function_call") => &["type", "call_id", "name", "namespace", "arguments"],
                Some("custom_tool_call") => &["type", "call_id", "name", "input"],
                Some("function_call_output" | "custom_tool_call_output") => {
                    &["type", "call_id", "output"]
                }
                _ => &[],
            },
            _ => &[],
        };
        row["payload"] = pick(&p, keys)?;
        return Ok(row);
    }
    let mut row = pick(
        &raw,
        &[
            "type",
            "parentSessionId",
            "forkedFromSessionId",
            "continuedInSessionId",
            "parentUuid",
            "uuid",
        ],
    )?;
    if let Some(message) = raw.get("message") {
        if let Ok(message) = serde_json::from_str::<Object<'_>>(message.get()) {
            if let Some(content) = message.get("content") {
                if let Ok(blocks) = serde_json::from_str::<Vec<&RawValue>>(content.get()) {
                    let mut kept = Vec::new();
                    for block in blocks {
                        let Ok(block) = serde_json::from_str::<Object<'_>>(block.get()) else {
                            continue;
                        };
                        match text(&block, "type").as_deref() {
                            Some("tool_use")
                                if matches!(
                                    text(&block, "name").as_deref(),
                                    Some("SendMessage" | "Agent" | "Task")
                                ) =>
                            {
                                kept.push(pick(&block, &["type", "id", "name", "input"])?)
                            }
                            Some("tool_result") => {
                                kept.push(pick(&block, &["type", "tool_use_id", "content"])?)
                            }
                            _ => {}
                        }
                    }
                    if !kept.is_empty() {
                        row["message"] = serde_json::json!({"content": kept});
                        if let Some(result) = raw.get("toolUseResult") {
                            row["toolUseResult"] = serde_json::from_str(result.get())?;
                        }
                    }
                }
            }
        }
    }
    Ok(row)
}
fn extract(e: &CandidateRef, links: &mut Links) -> Result<(), TransferError> {
    if e.source == "grok" {
        let Some(path) = &e.summary_path else {
            return Ok(());
        };
        let root = path.parent().unwrap();
        match grok_tools::rows(root) {
            Ok(rows) => {
                links.optional = grok_tools::references(&rows);
                links.required.extend(
                    grok_tools::required_references(&rows)
                        .into_iter()
                        .map(|id| (id, "agent_tool".into())),
                );
            }
            Err(error) => links.errors.push((error.code, error.message)),
        }
        let row: Value = serde_json::from_slice(&fs::read(path)?)?;
        links.alias = row["agent_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if let Some(id) = row["parent_session_id"].as_str().filter(|s| !s.is_empty()) {
            links.required.insert((id.into(), "fork".into()));
        }
        let agents = root.join("subagents");
        if agents.is_dir() {
            for entry in fs::read_dir(agents)? {
                let path = entry?.path().join("meta.json");
                if !path.is_file() {
                    continue;
                }
                match fs::read(path)
                    .map_err(TransferError::from)
                    .and_then(|raw| Ok(serde_json::from_slice::<Value>(&raw)?))
                {
                    Ok(row) => {
                        for field in ["child_session_id", "parent_session_id"] {
                            if let Some(id) = row[field].as_str().filter(|s| !s.is_empty()) {
                                links.required.insert((id.into(), "subagent".into()));
                            }
                        }
                    }
                    Err(error) => links.errors.push((error.code, error.message)),
                }
            }
        }
        return Ok(());
    }
    if !matches!(e.source, "claude" | "codex") {
        return Ok(());
    }
    let raw = match fs::read(&e.data) {
        Ok(raw) => raw,
        Err(_) if e.source == "codex" => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut claude_rows = Vec::new();
    let empty = BTreeMap::new();
    for line in raw.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        super::coordination::check()?;
        let row: Value = match relationship_row(line, e.source) {
            Ok(row) => row,
            Err(error) => {
                if e.source == "claude" {
                    links.errors.push(("move_format".into(), error.to_string()));
                }
                continue;
            }
        };
        if e.source == "codex" {
            super::codex_tools::collect_call(&row, &mut links.calls);
            links.required.extend(
                super::codex_tools::references(&row, &empty)
                    .into_iter()
                    .map(|id| (id, "agent_tool".into())),
            );
            let p = &row["payload"];
            if row["type"] == "response_item"
                && matches!(
                    p["type"].as_str(),
                    Some("function_call_output" | "custom_tool_call_output")
                )
            {
                if let Some(call) = p["call_id"].as_str() {
                    let ids = super::code_mode::result_references(&p["output"]);
                    if !ids.is_empty() {
                        links.outputs.entry(call.into()).or_default().extend(ids);
                    }
                }
            }
        } else {
            for field in [
                "parentSessionId",
                "forkedFromSessionId",
                "continuedInSessionId",
            ] {
                if let Some(id) = row[field].as_str().filter(|s| !s.is_empty()) {
                    links.required.insert((
                        id.into(),
                        if row["type"] == "fork-context-ref" {
                            "history_base"
                        } else {
                            "fork"
                        }
                        .into(),
                    ));
                }
            }
            if row.get("parentUuid").is_some() {
                if let Some(id) = row["uuid"].as_str().filter(|s| !s.is_empty()) {
                    links.messages.insert(id.into());
                }
            }
            claude_rows.push(row);
        }
    }
    if e.source == "claude" {
        let tools = claude_tools::links(&claude_rows);
        links.optional = tools.requests;
        links.required.extend(
            tools
                .resumed
                .into_values()
                .map(|id| (id, "agent_tool".into())),
        );
    }
    Ok(())
}
