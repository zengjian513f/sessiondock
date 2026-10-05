//! Durable transfer identities and read-only external-link resolution.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Link {
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub host: String,
    pub sid: String,
}
#[derive(Deserialize)]
pub struct Request {
    pub links: Vec<Link>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub created_ms: u64,
    pub mode: String,
    pub phase: String,
    pub source_node: String,
    pub target_node: String,
    pub mappings: BTreeMap<String, String>,
    #[serde(default)]
    pub legacy_members: BTreeSet<String>,
    #[serde(default)]
    pub legacy: bool,
}
impl Record {
    pub fn next(&self, spec: &str) -> Option<String> {
        if self.phase != "complete" {
            return None;
        }
        if let Some((owner, agent)) = spec.split_once("/agent:") {
            let source = owner.split_once(':')?.0;
            let target_owner = self.mappings.get(owner)?;
            let target_agent = self
                .mappings
                .get(&format!("{source}:{agent}"))
                .or_else(|| {
                    agent
                        .strip_prefix("agent-")
                        .and_then(|id| self.mappings.get(&format!("{source}:{id}")))
                })?;
            Some(format!(
                "{target_owner}/agent:{}",
                target_agent.split_once(':')?.1
            ))
        } else {
            self.mappings.get(spec).cloned()
        }
    }
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn identity(row: &Value) -> String {
    format!(
        "{}:{}",
        row["source"].as_str().unwrap_or(""),
        row["sid"].as_str().unwrap_or("")
    )
}
fn matches(row: &Value, spec: &str) -> bool {
    row["uid"] == spec || identity(row) == spec || row["sid"] == spec
}
/// Match the same native/record/agent link forms as the browser. Independent
/// copies remain ambiguous; only explicit continuation links collapse rows.
pub fn resolve_rows(rows: &[Value], spec: &str) -> Value {
    let (owner_spec, agent_spec) = spec
        .split_once("/agent:")
        .map_or((spec, None), |(a, b)| (a, Some(b)));
    let by_uid: BTreeMap<_, _> = rows
        .iter()
        .filter_map(|r| Some((r["uid"].as_str()?, r)))
        .collect();
    let mut found = BTreeMap::new();
    for mut row in rows.iter().filter(|r| matches(r, owner_spec)) {
        let mut seen = BTreeSet::new();
        while let Some(next) = row["continued_in"].as_str().and_then(|uid| by_uid.get(uid)) {
            if next["source"] != row["source"] || !seen.insert(row["uid"].as_str().unwrap_or("")) {
                break;
            }
            row = next;
        }
        if let Some(agent) = agent_spec {
            for item in row["agent_items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| a["id"] == agent || a["id"] == format!("agent-{agent}"))
            {
                found.insert(
                    format!(
                        "{}/agent:{}",
                        row["uid"].as_str().unwrap_or(""),
                        item["id"].as_str().unwrap_or("")
                    ),
                    (row, Some(item)),
                );
            }
        } else {
            found.insert(row["uid"].as_str().unwrap_or("").to_owned(), (row, None));
        }
    }
    if found.is_empty() && agent_spec.is_none() {
        let (source, id) = spec
            .split_once(':')
            .map_or((None, spec), |(s, id)| (Some(s), id));
        for row in rows
            .iter()
            .filter(|r| source.is_none_or(|s| r["source"] == s))
        {
            for item in row["agent_items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| a["id"] == id || a["id"] == format!("agent-{id}"))
            {
                found.insert(
                    format!(
                        "{}/agent:{}",
                        row["uid"].as_str().unwrap_or(""),
                        item["id"].as_str().unwrap_or("")
                    ),
                    (row, Some(item)),
                );
            }
        }
    }
    if found.len() != 1 {
        return json!({"sid":spec,"status":if found.is_empty(){"missing"}else{"ambiguous"}});
    }
    let (row, agent) = found.values().next().unwrap();
    let canonical = agent.map_or_else(
        || identity(row),
        |a| format!("{}/agent:{}", identity(row), a["id"].as_str().unwrap_or("")),
    );
    json!({"sid":spec,"status":"found","session":{"sid":canonical,"uid":row["uid"],"agent":agent.map(|a| &a["id"]),"title":agent.and_then(|a| a.get("title").filter(|v| v.is_string())).unwrap_or(&row["title"])}})
}
