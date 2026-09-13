//! Strict JSON grammar without duplicating the lifecycle receipt schema.

use serde_json::Value;

use super::{Document, Error};

pub(super) fn encode(document: &Document) -> Result<Vec<u8>, Error> {
    document.validate()?;
    let mut encoded = serde_json::to_vec(document).map_err(|_| Error::Invalid)?;
    encoded.push(b'\n');
    strict_value(&encoded)?;
    Ok(encoded)
}

pub(super) fn decode(bytes: &[u8]) -> Result<Document, Error> {
    let mut raw = strict_value(bytes)?;
    let schema = raw.get("schema").and_then(Value::as_u64);
    let legacy = matches!(schema, Some(1..=4));
    if raw.get("format").and_then(Value::as_str) != Some("agenthub-lifecycle")
        || !matches!(schema, Some(1..=5))
    {
        return Err(Error::UnsupportedSchema);
    }
    if legacy {
        // Explicit backwards-compatible additions only. A file that already
        // contains a field introduced by a later schema fails closed.
        let rows = raw
            .get_mut("records")
            .and_then(Value::as_object_mut)
            .ok_or(Error::Invalid)?;
        for row in rows.values_mut() {
            let fields = row.as_object_mut().ok_or(Error::Invalid)?;
            if schema == Some(1)
                && (fields.contains_key("cancel_requested")
                    || fields.get("state").and_then(Value::as_str) == Some("cancel_requested"))
            {
                return Err(Error::Invalid);
            }
            if matches!(schema, Some(1 | 2)) && fields.contains_key("binding") {
                return Err(Error::Invalid);
            }
            // Schema 5: terminal-state time and the discard tombstone on every
            // record; method/evidence/time on a binding. Older files must not
            // carry them.
            if ["created_at", "finished_at", "discarded"]
                .iter()
                .any(|key| fields.contains_key(*key))
            {
                return Err(Error::Invalid);
            }
            if let Some(binding) = fields.get_mut("binding").and_then(Value::as_object_mut) {
                if ["method", "evidence", "bound_at"]
                    .iter()
                    .any(|key| binding.contains_key(*key))
                {
                    return Err(Error::Invalid);
                }
                binding.insert("method".into(), Value::from("operator"));
                binding.insert("evidence".into(), Value::Null);
                binding.insert("bound_at".into(), Value::Null);
            }
            if schema != Some(4) {
                // Schema 4: typed launch kind inside the spec plus the optional
                // server-minted session ID. Every older receipt was a fixed-argv
                // adapter launch without a declared native identity.
                if fields.contains_key("session_id") {
                    return Err(Error::Invalid);
                }
                let spec = fields
                    .get_mut("spec")
                    .and_then(Value::as_object_mut)
                    .ok_or(Error::Invalid)?;
                if spec.contains_key("launch") {
                    return Err(Error::Invalid);
                }
                spec.insert("launch".into(), serde_json::json!({"kind":"fixed"}));
                if schema == Some(1) {
                    fields.insert("cancel_requested".into(), Value::Bool(false));
                }
                if schema != Some(3) {
                    fields.insert("binding".into(), Value::Null);
                }
                fields.insert("session_id".into(), Value::Null);
            }
            fields.insert("created_at".into(), Value::Null);
            fields.insert("finished_at".into(), Value::Null);
            fields.insert("discarded".into(), Value::Bool(false));
        }
        raw["schema"] = Value::from(super::SCHEMA);
    }
    let mut document: Document = serde_json::from_value(raw.clone()).map_err(|_| Error::Invalid)?;
    document.validate()?;
    document.legacy = legacy;
    Ok(document)
}

fn strict_value(bytes: &[u8]) -> Result<Value, Error> {
    serde_json::from_slice(bytes).map_err(|_| Error::Invalid)
}
