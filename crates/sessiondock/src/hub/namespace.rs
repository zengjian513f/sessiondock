//! The hub's wire namespace (`federation.py` 31–113): every reference a node
//! hands out is scoped with the node id before it reaches the browser, and
//! unscoped again before it goes back to the node. Native CLI ids, message
//! text, tool arguments and file contents are never touched.
//!
//! Shapes: session uid `<source>:<nid>~<local tail>`; terminal name, trash id
//! and outbox epoch `<nid>~<local>`; media `src` `/api/nodes/<nid>/api/media/…`.

#[cfg(test)]
mod tests;

use std::fmt;

use serde_json::{Map, Value};

use super::identity::is_node_id;
use super::registry::Node;

/// Keys whose string value is a scoped session reference.
const REFERENCE_KEYS: [&str; 4] = ["uid", "from_uid", "to_uid", "continued_in"];
/// Subtrees copied verbatim: message payloads, tool input, file contents and
/// resolved paths may legitimately contain a key called `uid` or `src`.
const OPAQUE_KEYS: [&str; 6] = ["data", "content", "input", "arguments", "raw", "resolved"];
const MEDIA_PREFIX: &str = "/api/media/";

/// A reference that cannot be scoped or unscoped; the
/// text is the user-facing 400 body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamespaceError(&'static str);

impl NamespaceError {
    pub fn message(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for NamespaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for NamespaceError {}

/// Scope a local reference with `node`. `uid` references keep their
/// `<source>:` prefix in front of the node; an empty value stays empty.
pub fn qualify(node: &str, value: &str, uid: bool) -> Result<String, NamespaceError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    if uid {
        let Some((source, tail)) = value.split_once(':') else {
            return Err(NamespaceError("invalid session reference"));
        };
        return Ok(format!("{source}:{node}~{tail}"));
    }
    Ok(format!("{node}~{value}"))
}

/// Split a scoped reference into `(node id, local reference)`; the local
/// reference of a uid keeps its `<source>:` prefix.
pub fn split(value: &str, uid: bool) -> Result<(String, String), NamespaceError> {
    let (prefix, rest) = if uid {
        let Some((source, rest)) = value.split_once(':') else {
            return Err(NamespaceError("missing session source"));
        };
        (format!("{source}:"), rest)
    } else {
        (String::new(), value)
    };
    match rest.split_once('~') {
        Some((node, local)) if is_node_id(node) && !local.is_empty() => {
            Ok((node.to_string(), format!("{prefix}{local}")))
        }
        _ => Err(NamespaceError("missing or invalid machine reference")),
    }
}

/// `federation.public_payload`: scope the protocol fields of a node answer
/// for `path`. Signature of `registry::PublicPayload`, so it can be
/// installed with `Registry::with_public_payload`. A reference the node
/// itself could not scope (a uid without `<source>:`) is left as it is;
/// `try_public_payload` reports it.
pub fn public_payload(data: Value, node: &Node, path: &str) -> Value {
    rewrite(data, node, path, false).unwrap_or_else(|_| unreachable!("lenient rewrite never fails"))
}

/// `public_payload` that refuses a payload with an unscopable reference.
pub fn try_public_payload(data: Value, node: &Node, path: &str) -> Result<Value, NamespaceError> {
    rewrite(data, node, path, true)
}

fn rewrite(data: Value, node: &Node, path: &str, strict: bool) -> Result<Value, NamespaceError> {
    let nid = node.id.as_str();
    let scoped = walk(data, nid, strict)?;
    let Value::Object(mut result) = scoped else {
        return Ok(scoped);
    };
    if path == "/api/sessions" || path == "/api/search" {
        // `sessions` if present, otherwise `results`.
        let key = if result.contains_key("sessions") {
            "sessions"
        } else {
            "results"
        };
        if let Some(rows) = result.get_mut(key).and_then(Value::as_array_mut) {
            decorate_rows(rows, node, false, false)?;
        }
    }
    if path == "/api/live" {
        for key in ["uids", "tmux_uids"] {
            let mut scoped = Vec::new();
            if let Some(items) = result.get(key).and_then(Value::as_array) {
                for item in items {
                    scoped.push(match item.as_str() {
                        Some(text) => {
                            Value::String(lenient(qualify(nid, text, true), text, strict)?)
                        }
                        None => item.clone(),
                    });
                }
            }
            result.insert(key.to_string(), Value::Array(scoped));
        }
        let mut started = Map::new();
        if let Some(entries) = result.get("started_at").and_then(Value::as_object) {
            for (uid, value) in entries {
                started.insert(
                    lenient(qualify(nid, uid, true), uid, strict)?,
                    value.clone(),
                );
            }
        }
        result.insert("started_at".to_string(), Value::Object(started));
    }
    if path == "/api/term/list" {
        for key in ["sessions", "pending"] {
            if let Some(rows) = result.get_mut(key).and_then(Value::as_array_mut) {
                decorate_rows(rows, node, true, false)?;
            }
        }
    }
    if matches!(
        path,
        "/api/term/create" | "/api/term/takeover" | "/api/term/new-status"
    ) {
        decorate_row(&mut result, node, true, false)?;
        if let Some(session) = result
            .get_mut("session")
            .filter(|session| truthy(session))
            .and_then(Value::as_object_mut)
        {
            decorate_row(session, node, false, false)?;
        }
    }
    if path == "/api/bug-report"
        && let Some(worker) = result
            .get_mut("worker")
            .filter(|worker| truthy(worker))
            .and_then(Value::as_object_mut)
    {
        decorate_row(worker, node, true, false)?;
    }
    if path == "/api/trash"
        && let Some(items) = result.get_mut("items").and_then(Value::as_array_mut)
    {
        decorate_rows(items, node, false, true)?;
    }
    if (path.starts_with("/api/messages/") || path == "/api/watch")
        && let Some(meta) = result
            .get_mut("meta")
            .filter(|meta| truthy(meta))
            .and_then(Value::as_object_mut)
    {
        decorate_row(meta, node, false, false)?;
    }
    Ok(Value::Object(result))
}

/// Scope reference keys, media `src` and `epoch` everywhere
/// except inside opaque subtrees.
fn walk(value: Value, nid: &str, strict: bool) -> Result<Value, NamespaceError> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| walk(item, nid, strict))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => {
            let mut result = Map::with_capacity(map.len());
            for (key, value) in map {
                let value = match (&key[..], &value) {
                    (key, Value::String(text))
                        if REFERENCE_KEYS.contains(&key) && !text.is_empty() =>
                    {
                        Value::String(lenient(qualify(nid, text, true), text, strict)?)
                    }
                    ("src", Value::String(text)) if text.starts_with(MEDIA_PREFIX) => {
                        Value::String(format!("/api/nodes/{nid}{text}"))
                    }
                    ("epoch", Value::String(text)) => Value::String(qualify(nid, text, false)?),
                    (key, _) if OPAQUE_KEYS.contains(&key) => value,
                    _ => walk(value, nid, strict)?,
                };
                result.insert(key, value);
            }
            Ok(Value::Object(result))
        }
        other => Ok(other),
    }
}

/// In lenient mode an unscopable reference stays as the node sent it.
fn lenient(
    scoped: Result<String, NamespaceError>,
    original: &str,
    strict: bool,
) -> Result<String, NamespaceError> {
    match scoped {
        Ok(value) => Ok(value),
        Err(error) if strict => Err(error),
        Err(_) => Ok(original.to_string()),
    }
}

/// Stamp `node_id`/`node_name` on a row; a terminal row's
/// `name` and a trash row's `id` are scoped as well.
pub fn decorate_row(
    row: &mut Map<String, Value>,
    node: &Node,
    terminal: bool,
    trash: bool,
) -> Result<(), NamespaceError> {
    row.insert("node_id".to_string(), Value::String(node.id.clone()));
    row.insert("node_name".to_string(), Value::String(node.name.clone()));
    if terminal
        && let Some(name) = row
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
    {
        let scoped = qualify(&node.id, name, false)?;
        row.insert("name".to_string(), Value::String(scoped));
    }
    if trash
        && let Some(id) = row
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
    {
        let scoped = qualify(&node.id, id, false)?;
        row.insert("id".to_string(), Value::String(scoped));
    }
    Ok(())
}

/// `decorate_row` over every object in `rows`; other elements are skipped.
pub fn decorate_rows(
    rows: &mut [Value],
    node: &Node,
    terminal: bool,
    trash: bool,
) -> Result<(), NamespaceError> {
    for row in rows.iter_mut().filter_map(Value::as_object_mut) {
        decorate_row(row, node, terminal, trash)?;
    }
    Ok(())
}

/// Truthiness of a JSON value.
pub(super) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}
