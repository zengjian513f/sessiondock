//! Summary-derived Grok metadata. The transcript never overrides these fields,
//! and URL-decoded cwd is display metadata, not a filesystem authorization.

use chrono::Timelike;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn validate(summary: &Value) -> Result<(), String> {
    if !summary.is_object() {
        return Err("Grok summary.json 必须是 JSON 对象".to_owned());
    }
    if !summary["info"].is_null() && !summary["info"].is_object() {
        return Err("Grok summary.json 的 info 必须是对象或 null".to_owned());
    }
    for key in [
        "generated_title",
        "session_summary",
        "current_model_id",
        "agent_name",
    ] {
        if !summary[key].is_null() && !summary[key].is_string() {
            return Err(format!("Grok summary.json 的 {key} 必须是字符串或 null"));
        }
    }
    for key in ["id", "cwd"] {
        if !summary["info"][key].is_null() && !summary["info"][key].is_string() {
            return Err(format!(
                "Grok summary.json 的 info.{key} 必须是字符串或 null"
            ));
        }
    }
    Ok(())
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.is_empty())
}
fn normalized(value: &Value) -> Value {
    if !super::truthy(value) {
        return Value::Null;
    }
    let date = if let Some(number) = value.as_f64() {
        let millis = if number > 1e11 {
            number
        } else {
            number * 1000.0
        };
        if millis.is_finite() && millis >= i64::MIN as f64 && millis < i64::MAX as f64 {
            chrono::DateTime::from_timestamp_millis(millis.floor() as i64)
        } else {
            None
        }
    } else if let Some(text) = value.as_str() {
        chrono::DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|dt| dt.to_utc())
            .or_else(|| {
                ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
                    .iter()
                    .find_map(|format| {
                        chrono::NaiveDateTime::parse_from_str(text, format)
                            .ok()
                            .map(|dt| dt.and_utc())
                    })
            })
            .or_else(|| {
                chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                    .ok()
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
                    .map(|dt| dt.and_utc())
            })
    } else {
        None
    };
    date.map(|dt| json!(dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)))
        .unwrap_or(Value::Null)
}
fn clip(title: &str) -> String {
    let normalized = title
        .split(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.chars().count() > 110 {
        normalized.chars().take(110).collect::<String>() + "…"
    } else {
        normalized
    }
}

/// urllib.parse.unquote semantics: percent UTF-8 decoding with replacement for
/// invalid bytes, malformed percent escapes retained, and '+' stays literal.
fn unquote(name: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let bytes = name.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%'
            && offset + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[offset + 1]), hex(bytes[offset + 2]))
        {
            decoded.push((high << 4) | low);
            offset += 3;
        } else {
            decoded.push(bytes[offset]);
            offset += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub(super) fn metadata(path: &Path, summary: &Value, fallback: &str) -> Value {
    let directory = path.file_name().unwrap_or_default().to_string_lossy();
    let project = path
        .parent()
        .and_then(Path::file_name)
        .unwrap_or_default()
        .to_string_lossy();
    let sid = nonempty(&summary["info"]["id"]).unwrap_or(&directory);
    let default_title = directory.chars().take(8).collect::<String>();
    let title = nonempty(&summary["generated_title"])
        .or_else(|| nonempty(&summary["session_summary"]))
        .unwrap_or(&default_title);
    let cwd = nonempty(&summary["info"]["cwd"])
        .map(str::to_owned)
        .unwrap_or_else(|| unquote(&project));
    // The mtime fallback uses _iso(...timespec='seconds'), independently
    // for created and updated. Do not substitute native message timestamps.
    let fallback = chrono::DateTime::parse_from_rfc3339(fallback)
        .ok()
        .and_then(|date| date.with_nanosecond(0))
        .map(|date| {
            date.to_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        })
        .unwrap_or_else(|| fallback.to_owned());
    let mut created = normalized(&summary["created_at"]);
    let selected = if super::truthy(&summary["last_active_at"]) {
        &summary["last_active_at"]
    } else {
        &summary["updated_at"]
    };
    let mut updated = normalized(selected);
    if created.is_null() {
        created = json!(fallback);
    }
    if updated.is_null() {
        updated = json!(fallback);
    }
    json!({"sid":sid,"title":clip(title),"cwd":cwd,"created":created,"updated":updated,
        "model":summary["current_model_id"],"branch":summary["agent_name"]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn percent_decode_matches_unquote_not_form_encoding() {
        assert_eq!(
            unquote("%2Fworkspace%2F%E4%B8%AD%E6%96%87+literal%20space"),
            "/workspace/中文+literal space"
        );
        assert_eq!(unquote("bad%GG%2%FF%00"), "bad%GG%2�\0");
    }
    #[test]
    fn summary_fields_and_independent_timestamp_fallbacks() {
        let path = Path::new("/synthetic/%2Fcwd+name/session-abcdefgh");
        let meta = metadata(
            path,
            &json!({"created_at":"2026-09-11T10:00:00.123Z","last_active_at":"invalid","updated_at":"2026-09-11T11:00:00Z"}),
            "2026-09-12T12:00:00.999Z",
        );
        assert_eq!(meta["sid"], "session-abcdefgh");
        assert_eq!(meta["title"], "session-");
        assert_eq!(meta["cwd"], "/cwd+name");
        assert_eq!(meta["created"], "2026-09-11T10:00:00.123Z");
        assert_eq!(meta["updated"], "2026-09-12T12:00:00.000Z");
        let meta = metadata(
            path,
            &json!({"info":{"id":"native-id","cwd":"explicit-cwd"},"generated_title":" \t ","session_summary":"must not replace whitespace title","last_active_at":"","updated_at":"2026-09-11 12:00:00","current_model_id":"synthetic-model","agent_name":"synthetic-agent"}),
            "2026-09-12T12:00:00Z",
        );
        assert_eq!(meta["title"], "");
        assert_eq!(meta["cwd"], "explicit-cwd");
        assert_eq!(meta["sid"], "native-id");
        assert_eq!(meta["updated"], "2026-09-11T12:00:00.000Z");
        assert_eq!(meta["model"], "synthetic-model");
        assert_eq!(meta["branch"], "synthetic-agent");
        assert_eq!(
            metadata(
                path,
                &json!({"generated_title":"名".repeat(111)}),
                "2026-09-12T12:00:00Z"
            )["title"],
            "名".repeat(110) + "…"
        );
    }
    #[test]
    fn invalid_summary_shape_is_not_silently_empty() {
        for value in [
            json!([]),
            json!(null),
            json!({"info":[]}),
            json!({"info":{"id":1}}),
            json!({"generated_title":false}),
            json!({"agent_name":{}}),
        ] {
            assert!(validate(&value).is_err());
        }
        assert!(validate(&json!({"info":null})).is_ok());
    }
}
