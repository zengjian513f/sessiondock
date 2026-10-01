//! Apply typed identity changes at their original JSON byte locations.
//! Unchanged tokens, whitespace, escapes and line endings are copied verbatim.
use super::TransferError;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::{Value, value::RawValue};
use std::{collections::BTreeMap, fmt, ops::Range};

struct Object<'a>(Vec<(String, &'a RawValue)>);
impl<'de> Deserialize<'de> for Object<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Object<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = Vec::new();
                while let Some(field) = map.next_entry()? {
                    fields.push(field);
                }
                Ok(Object(fields))
            }
        }
        d.deserialize_map(Fields)
    }
}
struct Edit {
    range: Range<usize>,
    bytes: Vec<u8>,
}
fn shape() -> TransferError {
    TransferError::new("move_format", "身份替换意外改变了原生 JSON 结构")
}
fn string_end(raw: &[u8], start: usize) -> usize {
    let mut end = start + 1;
    while raw[end] != b'"' {
        end += if raw[end] == b'\\' { 2 } else { 1 };
    }
    end + 1
}

/// Decoded UTF-8 boundaries -> original JSON string byte boundaries.
fn string_boundaries(raw: &str) -> Result<BTreeMap<usize, usize>, TransferError> {
    let bytes = raw.as_bytes();
    let (mut cursor, mut decoded) = (1, 0);
    let mut boundaries = BTreeMap::from([(0, 1)]);
    while cursor < bytes.len() - 1 {
        let start = cursor;
        let length = if bytes[cursor] == b'\\' {
            cursor += if bytes[cursor + 1] == b'u' { 6 } else { 2 };
            if bytes[start + 1] == b'u' {
                let code =
                    u16::from_str_radix(&raw[start + 2..start + 6], 16).map_err(|_| shape())?;
                if (0xd800..=0xdbff).contains(&code) {
                    cursor += 6;
                }
            }
            serde_json::from_str::<String>(&format!("\"{}\"", &raw[start..cursor]))?.len()
        } else {
            let length = raw[cursor..].chars().next().ok_or_else(shape)?.len_utf8();
            cursor += length;
            length
        };
        decoded += length;
        boundaries.insert(decoded, cursor);
    }
    Ok(boundaries)
}
fn word(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}
fn tokens(text: &str) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut previous = None;
    for (offset, c) in text.char_indices() {
        let class = word(c);
        if previous.is_some_and(|old| old != class) {
            result.push(start..offset);
            start = offset;
        }
        previous = Some(class);
    }
    result.push(start..text.len());
    result
}
fn changed_text(old: &str, new: &str, start: usize, edits: &mut Vec<Edit>) {
    if old == new {
        return;
    }
    let prefix: usize = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let suffix: usize = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    edits.push(Edit {
        range: start + prefix..start + old.len() - suffix,
        bytes: new.as_bytes()[prefix..new.len() - suffix].to_vec(),
    });
}
fn string_edits(
    raw: &str,
    old: &str,
    new: &str,
    base: usize,
    edits: &mut Vec<Edit>,
) -> Result<(), TransferError> {
    let mut decoded_edits = Vec::new();
    if let (Ok(before), Ok(after)) = (
        serde_json::from_str::<Value>(old),
        serde_json::from_str::<Value>(new),
    ) && (before.is_object() || before.is_array())
        && (after.is_object() || after.is_array())
    {
        // Native tools sometimes wrap JSON in a string. Preserve both the
        // inner document layout and the outer string's escape spelling.
        collect(old.as_bytes(), &before, &after, &mut decoded_edits)?;
    } else {
        let (left, right) = (tokens(old), tokens(new));
        if left.len() == right.len() {
            for (a, b) in left.iter().zip(&right) {
                changed_text(
                    &old[a.clone()],
                    &new[b.clone()],
                    a.start,
                    &mut decoded_edits,
                );
            }
        } else {
            changed_text(old, new, 0, &mut decoded_edits);
        }
    }
    if decoded_edits.is_empty() {
        return Ok(());
    }
    let boundaries = string_boundaries(raw)?;
    for edit in decoded_edits {
        let replacement = std::str::from_utf8(&edit.bytes).map_err(|_| shape())?;
        let encoded = serde_json::to_vec(replacement)?;
        edits.push(Edit {
            range: base + boundaries.get(&edit.range.start).ok_or_else(shape)?
                ..base + boundaries.get(&edit.range.end).ok_or_else(shape)?,
            bytes: encoded[1..encoded.len() - 1].to_vec(),
        });
    }
    Ok(())
}
fn walk(
    document: &[u8],
    raw: &RawValue,
    old: &Value,
    new: &Value,
    edits: &mut Vec<Edit>,
) -> Result<(), TransferError> {
    if old == new {
        return Ok(());
    }
    let offset = raw.get().as_ptr() as usize - document.as_ptr() as usize;
    match (old, new) {
        (Value::Object(before), Value::Object(after)) => {
            if before.len() != after.len() {
                return Err(shape());
            }
            let removed: Vec<_> = before
                .keys()
                .filter(|key| !after.contains_key(*key))
                .collect();
            let added: Vec<_> = after
                .keys()
                .filter(|key| !before.contains_key(*key))
                .collect();
            let renamed: BTreeMap<_, _> = removed.into_iter().zip(added).collect();
            let fields: Object<'_> = serde_json::from_str(raw.get())?;
            let bytes = raw.get().as_bytes();
            let mut cursor = 1;
            for (index, (key, value)) in fields.0.iter().enumerate() {
                while bytes[cursor] != b'"' {
                    cursor += 1;
                }
                let end = string_end(bytes, cursor);
                let key_start = cursor;
                cursor =
                    value.get().as_ptr() as usize - raw.get().as_ptr() as usize + value.get().len();
                // serde uses the last occurrence of a repeated object key.
                if fields.0[index + 1..].iter().any(|(later, _)| later == key) {
                    continue;
                }
                let next_key = renamed.get(key).copied().unwrap_or(key);
                if next_key != key {
                    string_edits(
                        &raw.get()[key_start..end],
                        key,
                        next_key,
                        offset + key_start,
                        edits,
                    )?;
                }
                walk(
                    document,
                    value,
                    before.get(key).ok_or_else(shape)?,
                    after.get(next_key).ok_or_else(shape)?,
                    edits,
                )?;
            }
        }
        (Value::Array(before), Value::Array(after)) => {
            if before.len() != after.len() {
                return Err(shape());
            }
            let values: Vec<&RawValue> = serde_json::from_str(raw.get())?;
            for ((value, old), new) in values.iter().zip(before).zip(after) {
                walk(document, value, old, new, edits)?;
            }
        }
        (Value::String(before), Value::String(after)) => {
            string_edits(raw.get(), before, after, offset, edits)?
        }
        (Value::Number(_), Value::Number(_)) => edits.push(Edit {
            range: offset..offset + raw.get().len(),
            bytes: serde_json::to_vec(new)?,
        }),
        _ => return Err(shape()),
    }
    Ok(())
}
fn collect(
    raw: &[u8],
    old: &Value,
    new: &Value,
    edits: &mut Vec<Edit>,
) -> Result<(), TransferError> {
    let value: &RawValue = serde_json::from_slice(raw)?;
    walk(raw, value, old, new, edits)
}
pub(super) fn rewrite(raw: &[u8], next: &Value) -> Result<Vec<u8>, TransferError> {
    let original: Value = serde_json::from_slice(raw)?;
    let mut edits = Vec::new();
    collect(raw, &original, next, &mut edits)?;
    edits.sort_by_key(|edit| edit.range.start);
    let mut result = Vec::with_capacity(raw.len());
    let mut cursor = 0;
    for edit in edits {
        if edit.range.start < cursor {
            return Err(shape());
        }
        result.extend_from_slice(&raw[cursor..edit.range.start]);
        result.extend_from_slice(&edit.bytes);
        cursor = edit.range.end;
    }
    result.extend_from_slice(&raw[cursor..]);
    Ok(result)
}

pub(super) fn rewrite_text(raw: &str, next: &Value) -> Result<String, TransferError> {
    String::from_utf8(rewrite(raw.as_bytes(), next)?).map_err(|_| shape())
}
