//! Strict JSON grammar without duplicating either provider's persisted schema.

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::{Document, Error};

pub(super) fn encode(document: &Document) -> Result<Vec<u8>, Error> {
    document.validate()?;
    let mut encoded = serde_json::to_vec(document).map_err(|_| Error::Invalid)?;
    encoded.push(b'\n');
    // Check the same grammar on write and read without limiting receipt count.
    strict_value(&encoded)?;
    Ok(encoded)
}

pub(super) fn decode(bytes: &[u8]) -> Result<Document, Error> {
    let raw = strict_value(bytes)?;
    if raw.get("format").and_then(Value::as_str) != Some("sessiondock-delivery")
        || raw.get("schema").and_then(Value::as_u64) != Some(1)
    {
        return Err(Error::UnsupportedSchema);
    }
    let document: Document = serde_json::from_value(raw.clone()).map_err(|_| Error::Invalid)?;
    document.validate()?;
    Ok(document)
}

fn strict_value(bytes: &[u8]) -> Result<Value, Error> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = Seed
        .deserialize(&mut deserializer)
        .map_err(|_| Error::Invalid)?;
    deserializer.end().map_err(|_| Error::Invalid)?;
    Ok(value)
}

struct Seed;

impl<'de> DeserializeSeed<'de> for Seed {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed {
    type Value = Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON")
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
        while let Some(value) = sequence.next_element_seed(Seed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            let value = object.next_value_seed(Seed)?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn strict_grammar_has_no_total_node_quota() {
        let bytes = serde_json::to_vec(&vec![false; 500_001]).unwrap();
        assert_eq!(
            super::strict_value(&bytes)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            500_001
        );
    }
}
