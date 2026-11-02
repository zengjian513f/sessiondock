//! Connection-local SDK transport. Persistent/native state stays complete.
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Change {
    Value {
        value: Value,
    },
    Object {
        fields: Vec<(String, Change)>,
        remove: Vec<String>,
    },
    Array {
        fields: Vec<(usize, Change)>,
    },
    Splice {
        start: usize,
        delete: usize,
        values: Vec<Value>,
    },
    Text {
        start: usize,
        delete: usize,
        text: String,
    },
}

fn diff(before: &Value, after: &Value) -> Option<Change> {
    if before == after {
        return None;
    }
    Some(match (before, after) {
        (Value::Object(old), Value::Object(new)) => Change::Object {
            fields: new
                .iter()
                .filter_map(|(key, value)| {
                    let patch = match old.get(key) {
                        Some(previous) => diff(previous, value)?,
                        None => Change::Value {
                            value: value.clone(),
                        },
                    };
                    Some((key.clone(), patch))
                })
                .collect(),
            remove: old
                .keys()
                .filter(|key| !new.contains_key(*key))
                .cloned()
                .collect(),
        },
        (Value::Array(old), Value::Array(new)) if old.len() == new.len() => Change::Array {
            fields: old
                .iter()
                .zip(new)
                .enumerate()
                .filter_map(|(index, (a, b))| diff(a, b).map(|patch| (index, patch)))
                .collect(),
        },
        (Value::Array(old), Value::Array(new)) => {
            let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
            let suffix = old[prefix..]
                .iter()
                .rev()
                .zip(new[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            Change::Splice {
                start: prefix,
                delete: old.len() - prefix - suffix,
                values: new[prefix..new.len() - suffix].to_vec(),
            }
        }
        (Value::String(old), Value::String(new)) => {
            let mut prefix = old
                .bytes()
                .zip(new.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            while !old.is_char_boundary(prefix) {
                prefix -= 1;
            }
            let mut suffix = old[prefix..]
                .bytes()
                .rev()
                .zip(new[prefix..].bytes().rev())
                .take_while(|(a, b)| a == b)
                .count();
            while !old.is_char_boundary(old.len() - suffix)
                || !new.is_char_boundary(new.len() - suffix)
            {
                suffix -= 1;
            }
            // JavaScript slices UTF-16, not UTF-8 bytes or Unicode scalars.
            Change::Text {
                start: old[..prefix].encode_utf16().count(),
                delete: old[prefix..old.len() - suffix].encode_utf16().count(),
                text: new[prefix..new.len() - suffix].to_owned(),
            }
        }
        _ => Change::Value {
            value: after.clone(),
        },
    })
}

pub(super) struct Encoder {
    enabled: bool,
    previous: Option<Value>,
    version: u64,
}

impl Encoder {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            previous: None,
            version: 0,
        }
    }

    pub(super) fn encode(&mut self, mut snapshot: Value) -> Value {
        if !self.enabled || snapshot["type"] != "snapshot" {
            return snapshot;
        }
        let raw = snapshot["raw"].take();
        let change = self.previous.as_ref().map(|old| diff(old, &raw));
        snapshot["snapshot_version"] = json!(self.version + 1);
        if let Some(change) = change {
            snapshot["type"] = json!("snapshot_delta");
            snapshot["snapshot_base"] = json!(self.version);
            // The existing Hub namespace treats raw as opaque for both full
            // snapshots and patches. Literal tool fields/strings stay literal.
            snapshot["raw"] = json!(change);
        } else {
            snapshot["raw"] = raw.clone();
        }
        self.previous = Some(raw);
        self.version += 1;
        snapshot
    }
}
