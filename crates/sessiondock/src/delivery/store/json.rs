//! Strict JSON grammar without duplicating either provider's persisted schema.

use std::cell::Cell;

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::{Document, Error, MAX_BYTES};

const MAX_NODES: usize = 500_000;
const MAX_DEPTH: usize = 64;

pub(super) fn encode(document: &Document) -> Result<Vec<u8>, Error> {
    document.validate()?;
    // A capped writer avoids allocating a potentially unbounded encoded body.
    struct Capped(Vec<u8>);
    impl std::io::Write for Capped {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > MAX_BYTES - 1 {
                return Err(std::io::Error::other("delivery byte limit"));
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
    let raw = strict_value(bytes)?;
    if raw.get("format").and_then(Value::as_str) != Some("agenthub-delivery")
        || raw.get("schema").and_then(Value::as_u64) != Some(1)
    {
        return Err(Error::UnsupportedSchema);
    }
    let document: Document = serde_json::from_value(raw.clone()).map_err(|_| Error::Invalid)?;
    document.validate()?;
    // Semantic equality ignores JSON field order/whitespace, but rejects any
    // unknown nested field or omitted Option field ignored/defaulted by serde.
    if serde_json::to_value(&document).map_err(|_| Error::Invalid)? != raw {
        return Err(Error::Invalid);
    }
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
            return Err(serde::de::Error::custom("delivery structure limit"));
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
                return Err(serde::de::Error::custom("duplicate delivery object key"));
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
