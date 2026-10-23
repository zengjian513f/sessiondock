//! Read-only V3 history observation. Identity still comes from the ACP catalog.
use super::{Catalog, FORMAT, publish, save};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::SystemTime};

pub(super) type Version = (Stamp, Stamp);
type Stamp = (u64, Option<SystemTime>);
fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()))
}

impl Catalog {
    pub(super) fn observe_native(&self) -> Result<(), String> {
        // Only bucket/session level: never descend into snapshots or workspaces.
        let buckets = match std::fs::read_dir(&self.native_root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        for bucket in buckets.flatten() {
            let Ok(dirs) = std::fs::read_dir(bucket.path()) else {
                continue;
            };
            for dir in dirs.flatten() {
                let sid = dir.file_name().to_string_lossy().into_owned();
                let guard = self.manifest.lock().unwrap();
                let Some(entry) = guard
                    .entries
                    .get(&sid)
                    .filter(|e| !e.deleted && e.host_key.is_none())
                else {
                    continue;
                };
                let summary_path = dir.path().join("session.json");
                let messages_path = dir.path().join("messages.jsonl");
                let Some(version) = stamp(&summary_path).zip(stamp(&messages_path)) else {
                    continue;
                };
                let mut observed = self.native_observed.lock().unwrap();
                if observed.get(&messages_path) == Some(&version) {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&summary_path) else {
                    continue;
                };
                let Ok(meta) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                if meta["id"] != sid || meta["dataModelVersion"] != 1 {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&messages_path) else {
                    continue;
                };
                // An unfinished final record is retried on the next observation.
                let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
                let records = bytes[..end]
                    .split(|b| *b == b'\n')
                    .filter(|line| !line.is_empty())
                    .map(serde_json::from_slice::<Value>)
                    .collect::<Result<Vec<_>, _>>();
                let Ok(records) = records else { continue };
                if stamp(&summary_path).zip(stamp(&messages_path)) != Some(version) {
                    continue;
                }
                let items = project(&records);
                let mut out = Vec::new();
                for item in items {
                    serde_json::to_writer(&mut out, &json!({"format":FORMAT,"item":item}))
                        .map_err(|e| e.to_string())?;
                    out.push(b'\n');
                }
                // The catalog lock also excludes SDK binding/projection publication.
                publish(&entry.path.join("messages.jsonl"), &out)?;
                let path = entry.path.join("summary.json");
                let mut summary: Value =
                    serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                summary["history_synced"] = json!(true);
                summary["native_observed"] = json!(true);
                summary["native_status"] = meta["status"].clone();
                for (key, native) in [
                    ("model", "modelId"),
                    ("title", "title"),
                    ("created", "createdAt"),
                    ("updated", "lastModifiedAt"),
                ] {
                    if !meta[native].is_null() {
                        summary["session"][key] = meta[native].clone();
                    }
                }
                save(&path, &summary)?;
                observed.insert(messages_path, version);
            }
        }
        Ok(())
    }
}

fn project(records: &[Value]) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    let mut tools = BTreeMap::<String, usize>::new();
    for record in records {
        let p = &record["payload"];
        let id = record["id"].clone();
        let timestamp = record["timestamp"].clone();
        match p["type"].as_str().unwrap_or_default() {
            "user" | "assistant" => {
                let role = if p["operationType"] == "Reasoning" {
                    "thinking"
                } else {
                    p["type"].as_str().unwrap()
                };
                items.push(json!({"id":id,"message_id":id,"role":role,"text":p["content"],"timestamp":timestamp}));
            }
            "tool_call" => {
                let call = p["toolCallId"].as_str().unwrap_or_default();
                let item = json!({"id":id,"role":"tool","toolCallId":call,"tool_name":p["toolName"],"name":p["toolName"],
                    "tool_input":p["args"],"input":p["args"],"status":p["status"],"kind":p["kind"],"timestamp":timestamp});
                if let Some(index) = tools.get(call) {
                    items[*index] = item;
                } else {
                    tools.insert(call.to_owned(), items.len());
                    items.push(item);
                }
            }
            "tool_result" => {
                if let Some(index) = p["toolCallId"].as_str().and_then(|id| tools.get(id)) {
                    let item = &mut items[*index];
                    item["tool_result"] = p["content"].clone();
                    item["result"] = p["content"].clone();
                    item["error"] = json!(p["success"] == false);
                    item["status"] = json!(if p["success"] == false {
                        "failed"
                    } else {
                        "completed"
                    });
                }
            }
            _ => {}
        }
    }
    items
}
