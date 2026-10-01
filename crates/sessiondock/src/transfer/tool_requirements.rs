//! Persisted definitions identify executor dependencies, not runnable tools.
use super::{ClonePlan, Native, TransferError};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader},
};

fn name(value: &Value, namespace: Option<&str>, tools: &mut BTreeSet<String>) {
    if let Some(name) = value["name"].as_str().filter(|s| !s.is_empty()) {
        tools.insert(match namespace.filter(|s| !s.is_empty()) {
            Some(namespace) => format!("{namespace}.{name}"),
            None => name.into(),
        });
    }
}

pub fn collect(plan: &ClonePlan, native: &Native) -> Result<BTreeSet<String>, TransferError> {
    let mut tools = BTreeSet::new();
    for table in native
        .databases
        .iter()
        .flat_map(|db| &db.tables)
        .filter(|table| table.name == "thread_dynamic_tools")
    {
        for row in &table.rows {
            name(
                &serde_json::to_value(row)?,
                row.get("namespace").and_then(Value::as_str),
                &mut tools,
            );
        }
    }
    // Older histories may have no database projection. Only the native session
    // metadata is authoritative; never interpret a user's tool-looking text.
    for file in &plan.files {
        for line in BufReader::new(File::open(&file.source)?).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let row: Value = serde_json::from_str(&line)?;
            if row["type"] != "session_meta" {
                continue;
            }
            if let Some(definitions) = row["payload"]["dynamic_tools"].as_array() {
                for definition in definitions {
                    if definition["type"] == "namespace" {
                        if let Some(children) = definition["tools"].as_array() {
                            for child in children {
                                name(child, definition["name"].as_str(), &mut tools);
                            }
                        }
                    } else {
                        name(definition, definition["namespace"].as_str(), &mut tools);
                    }
                }
            }
            break;
        }
    }
    Ok(tools)
}
