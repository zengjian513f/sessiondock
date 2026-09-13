//! Strict JSON grammar without duplicating the lifecycle receipt schema.

use std::cell::Cell;

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::{Document, Error, MAX_BYTES};

const MAX_NODES: usize = 16_384;
const MAX_DEPTH: usize = 16;

pub(super) fn encode(document: &Document) -> Result<Vec<u8>, Error> {
    document.validate()?;
    // A capped writer avoids allocating a potentially unbounded encoded body.
    struct Capped(Vec<u8>);
    impl std::io::Write for Capped {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > MAX_BYTES - 1 {
                return Err(std::io::Error::other("lifecycle byte limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut encoded = Capped(Vec::new());
    serde_json::to_writer(&mut encoded, document).map_err(|_| Error::Limit)?;
    encoded.0.push(b'\n');
    // Apply identical structural budgets on reads and writes. No successfully
    // persisted document is rejected on restart merely because of node count.
    strict_value(&encoded.0)?;
    Ok(encoded.0)
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
    // Semantic equality ignores JSON field order/whitespace, but rejects any
    // unknown nested field or omitted Option field ignored/defaulted by serde.
    if serde_json::to_value(&document).map_err(|_| Error::Invalid)? != raw {
        return Err(Error::Invalid);
    }
    document.legacy = legacy;
    Ok(document)
}

fn strict_value(bytes: &[u8]) -> Result<Value, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::Limit);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let nodes = Cell::new(0usize);
    let value = Seed {
        nodes: &nodes,
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| Error::Invalid)?;
    deserializer.end().map_err(|_| Error::Invalid)?;
    Ok(value)
}

struct Seed<'a> {
    nodes: &'a Cell<usize>,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.nodes.set(self.nodes.get() + 1);
        if self.depth > MAX_DEPTH || self.nodes.get() > MAX_NODES {
            return Err(serde::de::Error::custom("lifecycle structure limit"));
        }
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded JSON with unique object keys")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(Seed {
            nodes: self.nodes,
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(serde::de::Error::custom("duplicate lifecycle object key"));
            }
            let value = object.next_value_seed(Seed {
                nodes: self.nodes,
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
