//! Optional identity-checked envelope. An old host rejects this *operation*
//! before dispatch instead of ignoring an unfamiliar field on a legacy write.
use serde_json::{Value, json};

#[path = "native_binding.rs"]
pub mod binding;

pub const OP: &str = "guarded_v1";
pub const ERROR: &str = "instance guard rejected";
pub const LAUNCH_OP: &str = "launch_guard_v1";
pub const LAUNCH_ERROR: &str = "launch guard rejected";

struct LaunchIdentity<'a> {
    instance: &'a str,
    source: &'a str,
    launch: &'a str,
}

pub struct Prepared<'a> {
    pub request: &'a Value,
    pub instance: Option<&'a str>,
    launch: Option<LaunchIdentity<'a>>,
}

impl Prepared<'_> {
    pub fn acknowledge(&self, reply: &mut Value) {
        acknowledge(reply, self.instance);
        if let Some(identity) = &self.launch
            && reply["ok"] == true
        {
            reply["launch_guard"] = json!({"version":1,"instance_id":identity.instance,
                "source":identity.source,"launch_id":identity.launch});
        }
    }
}

fn identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
}
fn fields(value: &Value, allowed: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
}
fn optional_bool(value: &Value, key: &str) -> bool {
    value.get(key).is_none_or(Value::is_boolean)
}
fn dimension(value: &Value, key: &str) -> Option<u16> {
    u16::try_from(value.get(key)?.as_u64()?)
        .ok()
        .filter(|number| *number != 0)
}
fn name(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 128
        || value.starts_with('.')
        || value.ends_with('.')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    {
        return false;
    }
    let stem = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}

fn operation(request: &Value) -> bool {
    let Some(op) = request["op"].as_str() else {
        return false;
    };
    match op {
        "info" | "cursor" => fields(request, &["op"]),
        "send" | "paste" => {
            fields(
                request,
                if op == "send" {
                    &["op", "text"]
                } else {
                    &["op", "text", "bracketed"]
                },
            ) && request["text"]
                .as_str()
                .is_some_and(|text| text.len() <= 1024 * 1024)
                && optional_bool(request, "bracketed")
        }
        "keys" => {
            fields(request, &["op", "keys"])
                && request["keys"].as_array().is_some_and(|keys| {
                    keys.len() <= 256
                        && keys.iter().all(|key| {
                            key.as_str()
                                .is_some_and(|key| !key.is_empty() && key.len() <= 256)
                        })
                })
        }
        "resize" | "attach" => {
            fields(
                request,
                if op == "resize" {
                    &["op", "cols", "rows"]
                } else {
                    &["op", "cols", "rows", "replay", "mode"]
                },
            ) && dimension(request, "cols")
                .zip(dimension(request, "rows"))
                .is_some()
                && optional_bool(request, "replay")
                && request
                    .get("mode")
                    .is_none_or(|mode| matches!(mode.as_str(), Some("bytes" | "grid")))
        }
        "grid_rows" => {
            fields(request, &["op", "from", "to"])
                && request.get("from").is_some_and(Value::is_u64)
                && request.get("to").is_some_and(Value::is_u64)
        }
        "capture" => {
            fields(request, &["op", "kind", "styled", "join", "lines"])
                && request
                    .get("kind")
                    .is_none_or(|kind| matches!(kind.as_str(), Some("screen" | "scrollback")))
                && optional_bool(request, "styled")
                && optional_bool(request, "join")
                && request
                    .get("lines")
                    .is_none_or(|lines| lines.as_u64().is_some_and(|n| n <= 100_000))
        }
        "rename" => fields(request, &["op", "to"]) && request["to"].as_str().is_some_and(name),
        "kill" => fields(request, &["op", "force"]) && optional_bool(request, "force"),
        _ => false,
    }
}

/// Called only after ordinary socket/token authentication and before any
/// dispatch, resize, replay subscription, or PTY write. Session metadata is
/// immutable for the lifetime of the same Session used for the operation.
pub fn prepare<'a>(request: &'a Value, metadata: &Value) -> Result<Prepared<'a>, &'static str> {
    if request["op"] == LAUNCH_OP {
        return prepare_launch(request, metadata);
    }
    if request["op"] != OP {
        return Ok(Prepared {
            request,
            instance: None,
            launch: None,
        });
    }
    if !fields(
        request,
        &[
            "op",
            "token",
            "expected_instance_id",
            "expected_source",
            "expected_sid",
            "expected_uid",
            "request",
        ],
    ) || request.get("token").is_some_and(|token| !token.is_string())
    {
        return Err(ERROR);
    }
    let instance = request["expected_instance_id"].as_str().ok_or(ERROR)?;
    let source = request["expected_source"].as_str().ok_or(ERROR)?;
    if instance.len() < 16
        || !identifier(instance, 128)
        || !matches!(source, "claude" | "codex" | "grok" | "opencode" | "agy")
        || metadata["instance_id"] != instance
        || metadata["source"] != source
    {
        return Err(ERROR);
    }
    let mut identities = 0;
    for (expected, key) in [("expected_sid", "sid"), ("expected_uid", "uid")] {
        if let Some(value) = request.get(expected) {
            let value = value.as_str().ok_or(ERROR)?;
            if !identifier(value, 256) || metadata[key] != value {
                return Err(ERROR);
            }
            if key == "uid"
                && (!value.starts_with(&format!("{source}:")) || value.len() <= source.len() + 1)
            {
                return Err(ERROR);
            }
            identities += 1;
        }
    }
    if identities == 0 || !operation(&request["request"]) {
        return Err(ERROR);
    }
    Ok(Prepared {
        request: &request["request"],
        instance: Some(instance),
        launch: None,
    })
}

/// A write-once binding is a separate native identity source. The immutable
/// launch metadata and legacy native declarations are never rewritten.
pub fn prepare_with_binding<'a>(
    request: &'a Value,
    metadata: &Value,
    binding: Option<&binding::NativeBinding>,
) -> Result<Prepared<'a>, &'static str> {
    if request["op"] == OP
        && let Some(binding) = binding
    {
        // Unlike legacy declarations, a new binding always declares both IDs.
        if !binding.matches_pending(metadata)
            || request["expected_sid"].as_str() != Some(binding.sid())
            || request["expected_uid"].as_str() != Some(binding.uid())
        {
            return Err(ERROR);
        }
        let mut effective = metadata.clone();
        effective["sid"] = json!(binding.sid());
        effective["uid"] = json!(binding.uid());
        return prepare(request, &effective);
    }
    prepare(request, metadata)
}

/// Launch identity deliberately needs no native SID/UID. It is an independent
/// immutable identity domain and never creates or upgrades native association.
fn prepare_launch<'a>(request: &'a Value, metadata: &Value) -> Result<Prepared<'a>, &'static str> {
    if !fields(
        request,
        &[
            "op",
            "token",
            "expected_instance_id",
            "expected_source",
            "expected_launch_id",
            "request",
        ],
    ) || request.get("token").is_some_and(|token| !token.is_string())
    {
        return Err(LAUNCH_ERROR);
    }
    let instance = request["expected_instance_id"]
        .as_str()
        .ok_or(LAUNCH_ERROR)?;
    let source = request["expected_source"].as_str().ok_or(LAUNCH_ERROR)?;
    let launch = request["expected_launch_id"].as_str().ok_or(LAUNCH_ERROR)?;
    if instance.len() < 16
        || !identifier(instance, 128)
        || launch.len() < 16
        || !identifier(launch, 128)
        || !matches!(
            source,
            "claude" | "codex" | "grok" | "opencode" | "agy" | "shell"
        )
        || metadata["instance_id"] != instance
        || metadata["source"] != source
        || metadata["launch_id"] != launch
        || !operation(&request["request"])
    {
        return Err(LAUNCH_ERROR);
    }
    Ok(Prepared {
        request: &request["request"],
        instance: None,
        launch: Some(LaunchIdentity {
            instance,
            source,
            launch,
        }),
    })
}

pub fn acknowledge(reply: &mut Value, instance: Option<&str>) {
    if let Some(instance) = instance
        && reply["ok"] == true
    {
        reply["instance_guard"] = json!({"version":1,"instance_id":instance});
    }
}
