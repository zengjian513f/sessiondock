//! Shared per-session accounting. A process is selected once, even when several
//! verified launch-chain entries refer to the same native session.
use crate::{Session, agent::Resources};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const FIELDS: &[&str] = &[
    "cpu_cores",
    "process_count",
    "gpu_count",
    "gpu_memory_bytes",
    "memory_pss_bytes",
    "memory_bandwidth_bytes_per_second",
    "disk_read_operations_per_second",
    "disk_write_operations_per_second",
    "nfs_read_operations_per_second",
    "nfs_write_operations_per_second",
    "disk_read_bytes_per_second",
    "disk_write_bytes_per_second",
    "network_receive_bytes_per_second",
    "network_send_bytes_per_second",
    "nfs_read_bytes_per_second",
    "nfs_write_bytes_per_second",
    "proc_storage_read_bytes_per_second",
    "proc_storage_write_bytes_per_second",
];
fn same(a: &Session, b: &Session) -> bool {
    a.node_id == b.node_id && a.source == b.source && a.sid == b.sid
}
fn unknown(reason: &str) -> Value {
    json!({"value":null,"status":"unavailable","reason":reason})
}

/// Unowned compute processes are evidence of incomplete attribution, never
/// evidence that any particular session owns those devices.
pub fn gpu_attribution_incomplete(resources: &Resources) -> bool {
    let bound: BTreeSet<_> = resources.bindings.iter().map(|b| &b.process).collect();
    resources.samples.iter().any(|sample| {
        !bound.contains(&sample.process)
            && sample.metrics.get("gpu_devices").is_some_and(|metric| {
                matches!(metric["status"].as_str(), Some("ok" | "partial"))
                    && metric["value"]
                        .as_array()
                        .is_some_and(|devices| !devices.is_empty())
            })
    })
}

pub fn mark_gpu_attribution_incomplete(metrics: &mut Value) {
    for key in ["gpu_count", "gpu_memory_bytes"] {
        let Some(metric) = metrics.get_mut(key) else {
            continue;
        };
        mark_incomplete(metric);
    }
}

fn mark_incomplete(metric: &mut Value) {
    if !metric.is_object() {
        return;
    }
    if metric["attribution_incomplete"] != true {
        let reason = metric["reason"].as_str().unwrap_or_default();
        metric["reason"] = json!(format!(
            "{reason}；存在未归属会话的 GPU 进程，仅统计已确认部分，不能据此确认零占用"
        ));
    }
    metric["attribution_incomplete"] = json!(true);
    if metric["value"].as_f64() == Some(0.0) {
        metric["value"] = Value::Null;
    }
    metric["status"] = json!(if metric["value"].is_null() {
        "unavailable"
    } else {
        "partial"
    });
}

fn sum<'a>(items: impl Iterator<Item = &'a Value>, fallback: &Value, devices: bool) -> Value {
    let mut count = 0;
    let mut observed = 0;
    let mut total = 0.0;
    let mut uuids = BTreeSet::new();
    let mut partial = false;
    let mut reasons = BTreeSet::new();
    let mut sampled_at: Option<f64> = None;
    let mut attribution_incomplete = false;
    for item in items {
        attribution_incomplete |= item["attribution_incomplete"] == true;
        count += 1;
        if !item["value"].is_null()
            && let Some(at) = item["sampled_at"].as_f64().filter(|at| at.is_finite())
        {
            sampled_at = Some(sampled_at.map_or(at, |old| old.min(at)));
        }
        if let Some(reason) = item["reason"].as_str() {
            reasons.insert(reason.to_owned());
        }
        if !matches!(item["status"].as_str(), Some("ok")) {
            partial = true;
        }
        if devices {
            if let Some(values) = item["value"].as_array() {
                observed += 1;
                uuids.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        } else if let Some(n) = item["value"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)
        {
            observed += 1;
            total += n;
        }
    }
    if count == 0 {
        let mut value = fallback.clone();
        if matches!(value["status"].as_str(), Some("ok" | "partial")) {
            value["value"] = json!(0);
        }
        return value;
    }
    if observed == 0 {
        let mut value = json!({"value":null,"status":"unavailable","reason":reasons.into_iter().collect::<Vec<_>>().join("; ")});
        if attribution_incomplete {
            value["attribution_incomplete"] = json!(true);
        }
        return value;
    }
    if devices {
        total = uuids.len() as f64;
    }
    let mut value = json!({"value":total,"sampled_at":sampled_at,"status":if partial || observed < count {"partial"}else{"ok"},"reason":reasons.into_iter().collect::<Vec<_>>().join("; ")});
    if attribution_incomplete {
        value["attribution_incomplete"] = json!(true);
        mark_incomplete(&mut value);
    }
    value
}

pub fn metrics(resources: &Resources, session: &Session, inclusive: bool) -> Value {
    let selected: BTreeSet<_> = resources
        .bindings
        .iter()
        .filter(|b| {
            same(&b.session, session)
                || (inclusive
                    && (b.initiator.as_ref().is_some_and(|s| same(s, session))
                        || b.launch_chain.iter().any(|l| same(&l.session, session))))
        })
        .map(|b| &b.process)
        .collect();
    let samples: BTreeMap<_, _> = resources
        .samples
        .iter()
        .filter(|s| selected.contains(&s.process))
        .map(|s| (&s.process, s))
        .collect();
    let absent = unknown("collector does not support this metric");
    let mut result = FIELDS
        .iter()
        .map(|field| {
            if *field == "process_count" {
                let value = if !selected.is_empty() && samples.is_empty() {
                    unknown("no current process samples for attributed bindings")
                } else {
                    json!({"value":samples.len(),"status":if samples.len() < selected.len() {"partial"} else {"ok"},"sampled_at":resources.sampled_at,"reason":"distinct attributed process incarnations with current samples"})
                };
                return ((*field).to_owned(), value);
            }
            let key = if *field == "gpu_count" {
                "gpu_devices"
            } else {
                field
            };
            let fallback = resources.metric_availability.get(key).unwrap_or(&absent);
            if *field == "memory_bandwidth_bytes_per_second" {
                // Group-native counters are summed once per owning session.
                // Inclusive launch chains select child groups, not each child PID.
                let mut owners = BTreeMap::new();
                owners.insert((&session.node_id, &session.source, &session.sid), session);
                for b in &resources.bindings {
                    if selected.contains(&b.process) {
                        owners.insert(
                            (&b.session.node_id, &b.session.source, &b.session.sid),
                            &b.session,
                        );
                    }
                }
                let mut value = sum(
                    owners.values().map(|owner| {
                        resources
                            .session_measurements
                            .iter()
                            .find(|m| same(&m.session, owner))
                            .and_then(|m| m.metrics.get(*field))
                            .unwrap_or(fallback)
                    }),
                    fallback,
                    false,
                );
                if value["sampled_at"].is_null() && !value["value"].is_null() {
                    value["sampled_at"] = json!(resources.sampled_at);
                }
                return ((*field).to_owned(), value);
            }
            let mut value = sum(
                samples
                    .values()
                    .map(|s| s.metrics.get(key).unwrap_or(&absent)),
                fallback,
                *field == "gpu_count",
            );
            if value["sampled_at"].is_null() && !value["value"].is_null() {
                value["sampled_at"] = json!(resources.sampled_at);
            }
            ((*field).to_owned(), value)
        })
        .collect::<serde_json::Map<_, _>>()
        .into();
    if gpu_attribution_incomplete(resources) {
        mark_gpu_attribution_incomplete(&mut result);
    }
    result
}

/// Totals keep missing machines visible through partial status; cards are
/// already distinct per execution machine, so their counts can be added.
pub fn totals(rows: &[Value]) -> Value {
    let absent = unknown("machine unavailable or collector unsupported");
    FIELDS
        .iter()
        .map(|key| {
            let value = sum(
                rows.iter()
                    .map(|row| row["metrics"].get(*key).unwrap_or(&absent)),
                &absent,
                false,
            );
            ((*key).to_owned(), value)
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

pub fn sessions(resources: &Resources) -> Vec<Value> {
    let mut owners = BTreeMap::new();
    for binding in &resources.bindings {
        let s = &binding.session;
        owners.entry((&s.node_id, &s.source, &s.sid)).or_insert(s);
    }
    owners
        .values()
        .map(|s| json!({"session":s,"metrics":metrics(resources,s,false)}))
        .collect()
}
