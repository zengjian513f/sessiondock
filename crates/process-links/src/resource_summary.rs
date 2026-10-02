//! Shared per-session accounting. A process is selected once, even when several
//! verified launch-chain entries refer to the same native session.
use crate::{Session, agent::Resources};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const FIELDS: &[&str] = &[
    "cpu_cores",
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
fn sum<'a>(items: impl Iterator<Item = &'a Value>, fallback: &Value, devices: bool) -> Value {
    let mut count = 0;
    let mut observed = 0;
    let mut total = 0.0;
    let mut uuids = BTreeSet::new();
    let mut partial = false;
    let mut reasons = BTreeSet::new();
    let mut sampled_at: Option<f64> = None;
    for item in items {
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
        return json!({"value":null,"status":"unavailable","reason":reasons.into_iter().collect::<Vec<_>>().join("; ")});
    }
    if devices {
        total = uuids.len() as f64;
    }
    json!({"value":total,"sampled_at":sampled_at,"status":if partial || observed < count {"partial"}else{"ok"},"reason":reasons.into_iter().collect::<Vec<_>>().join("; ")})
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
    FIELDS
        .iter()
        .map(|field| {
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
        .into()
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

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Resources {
        serde_json::from_value(json!({"version":1,"node_id":"execution","boot_id":"boot","sampled_at":1.0,"availability":"observed","method":"fixture","unavailable":[],
          "metric_availability":{"cpu_cores":{"status":"ok","value":null},"gpu_devices":{"status":"partial","value":null}},
          "samples":[
           {"process":{"pid":1,"start":100},"cpu_seconds":0,"rss_bytes":0,"threads":1,"read_bytes":null,"write_bytes":null,"metrics":{"cpu_cores":{"value":2,"status":"ok"},"gpu_devices":{"value":["GPU-1"],"status":"partial"}}},
           {"process":{"pid":2,"start":200},"cpu_seconds":0,"rss_bytes":0,"threads":1,"read_bytes":null,"write_bytes":null,"metrics":{"cpu_cores":{"value":3,"status":"ok"},"gpu_devices":{"value":["GPU-1"],"status":"partial"}}},
           {"process":{"pid":3,"start":301},"cpu_seconds":0,"rss_bytes":0,"threads":1,"read_bytes":null,"write_bytes":null,"metrics":{"cpu_cores":{"value":99,"status":"ok"}}}],
          "bindings":[
           {"process":{"pid":1,"start":100},"session":{"node_id":"owner","source":"codex","sid":"parent"},"first_observed_at":0},
           {"process":{"pid":2,"start":200},"session":{"node_id":"child","source":"claude","sid":"child"},"initiator":{"node_id":"owner","source":"codex","sid":"parent"},"first_observed_at":0,"launch_chain":[{"process":{"node_id":"owner","boot_id":"boot","process":{"pid":90,"start":9}},"session":{"node_id":"owner","source":"codex","sid":"parent"}}]},
           {"process":{"pid":3,"start":300},"session":{"node_id":"owner","source":"codex","sid":"parent"},"first_observed_at":0}]
        })).unwrap()
    }
    #[test]
    fn union_deduplicates_launch_chain_gpu_and_rejects_pid_reuse() {
        let mut data = fixture();
        data.samples[0].metrics.get_mut("cpu_cores").unwrap()["sampled_at"] = json!(10.0);
        data.samples[1].metrics.get_mut("cpu_cores").unwrap()["sampled_at"] = json!(20.0);
        let session = &data.bindings[0].session;
        assert_eq!(metrics(&data, session, false)["cpu_cores"]["value"], 2.0);
        let inclusive = metrics(&data, session, true);
        assert_eq!(inclusive["cpu_cores"]["value"], 5.0);
        assert_eq!(inclusive["cpu_cores"]["sampled_at"], 10.0);
        assert_eq!(inclusive["gpu_count"]["value"], 1.0);
        assert_eq!(inclusive["memory_pss_bytes"]["value"], Value::Null);
    }
    #[test]
    fn bandwidth_is_group_native_and_inclusive_deduplicates_owners() {
        let mut data = fixture();
        let parent = data.bindings[0].session.clone();
        let child = data.bindings[1].session.clone();
        data.bindings.push(data.bindings[1].clone());
        data.session_measurements = vec![
            crate::agent::SessionMeasurement {
                session: parent.clone(),
                metrics: BTreeMap::from([(
                    "memory_bandwidth_bytes_per_second".into(),
                    json!({"value":100,"status":"partial","sampled_at":12}),
                )]),
            },
            crate::agent::SessionMeasurement {
                session: child,
                metrics: BTreeMap::from([(
                    "memory_bandwidth_bytes_per_second".into(),
                    json!({"value":200,"status":"partial","sampled_at":10}),
                )]),
            },
        ];
        assert_eq!(
            metrics(&data, &parent, false)["memory_bandwidth_bytes_per_second"]["value"],
            100.0
        );
        let inclusive = metrics(&data, &parent, true);
        assert_eq!(
            inclusive["memory_bandwidth_bytes_per_second"]["value"],
            300.0
        );
        assert_eq!(
            inclusive["memory_bandwidth_bytes_per_second"]["sampled_at"],
            10.0
        );
        data.session_measurements.pop();
        assert_eq!(
            metrics(&data, &parent, true)["memory_bandwidth_bytes_per_second"]["status"],
            "partial"
        );
    }
    #[test]
    fn unavailable_machine_does_not_become_zero() {
        let rows = vec![
            json!({"metrics":{"cpu_cores":{"value":2,"status":"ok"}}}),
            json!({"status":"offline"}),
        ];
        let sum = totals(&rows);
        assert_eq!(sum["cpu_cores"]["value"], 2.0);
        assert_eq!(sum["cpu_cores"]["status"], "partial");
        assert_eq!(sum["gpu_count"]["value"], Value::Null);
    }
    #[test]
    fn empty_session_zero_only_for_supported_metrics() {
        let data = fixture();
        let mut s = data.bindings[0].session.clone();
        s.sid = "absent".into();
        let m = metrics(&data, &s, false);
        assert_eq!(m["cpu_cores"]["value"], 0);
        assert_eq!(m["memory_pss_bytes"]["value"], Value::Null);
    }
}
