//! Conservative lexical adapter for native code-mode agent calls. Only literal
//! identity arguments of recognized tools are rewritten; arbitrary JS is never
//! evaluated, and unsupported computed references remain a planning blocker.
use super::{TransferError, codex::IdentityMap};
use serde_json::Value;
#[derive(Debug)]
struct Token {
    start: usize,
    end: usize,
    text: String,
    string: bool,
}
fn tokens(code: &str) -> Vec<Token> {
    let b = code.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b.get(i..i + 2) == Some(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b.get(i..i + 2) == Some(b"/*") {
            i += 2;
            while i + 1 < b.len() && &b[i..i + 2] != b"*/" {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        let start = i;
        let string = matches!(b[i], b'\'' | b'"' | b'`');
        if string {
            let quote = b[i];
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i = (i + 2).min(b.len());
                    continue;
                }
                if b[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else if b[i].is_ascii_alphabetic() || b[i] == b'_' || b[i] == b'$' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'$')) {
                i += 1;
            }
        } else {
            i += 1;
            while !code.is_char_boundary(i) {
                i += 1;
            }
        }
        let raw = &code[start..i];
        let text = if string && raw.len() >= 2 {
            raw[1..raw.len() - 1].to_owned()
        } else {
            raw.to_owned()
        };
        out.push(Token {
            start,
            end: i,
            text,
            string,
        });
    }
    out
}
fn failure() -> TransferError {
    TransferError::new(
        "move_reference_unsupported",
        "code-mode 中存在无法静态解析的会话引用",
    )
}
pub(super) fn script(code: &str, map: &IdentityMap) -> Result<String, TransferError> {
    if !map.threads.keys().any(|id| code.contains(id)) {
        return Ok(code.into());
    }
    let ts = tokens(code);
    let mut edits = Vec::new();
    let mut accepted = Vec::new();
    for i in 2..ts.len().saturating_sub(2) {
        if ts[i].string
            || ts[i - 1].text != "."
            || ts[i - 2].text != "tools"
            || ts[i + 1].text != "("
            || ts[i + 2].text != "{"
        {
            continue;
        }
        if !matches!(
            ts[i].text.as_str(),
            "multi_agent_v1__wait_agent"
                | "multi_agent_v1__send_input"
                | "multi_agent_v1__close_agent"
                | "multi_agent_v1__resume_agent"
                | "multi_agent_v1__spawn_agent"
        ) {
            continue;
        }
        let mut depth = 1i32;
        let mut j = i + 3;
        while j < ts.len() && depth > 0 {
            if !ts[j].string && ts[j].text == "{" {
                depth += 1;
            }
            if !ts[j].string && ts[j].text == "}" {
                depth -= 1;
            }
            if depth == 1 && j + 2 < ts.len() && ts[j + 1].text == ":" {
                let key = &ts[j].text;
                if matches!(key.as_str(), "target" | "targets" | "id" | "ids") {
                    let mut k = j + 2;
                    let array = ts[k].text == "[";
                    if array {
                        k += 1;
                    }
                    while k < ts.len() {
                        if ts[k].string {
                            if let Some(new) = map.threads.get(&ts[k].text) {
                                edits.push((ts[k].start, ts[k].end, serde_json::to_string(new)?));
                                accepted.push((ts[k].start, ts[k].end));
                            }
                            if !array {
                                break;
                            }
                        } else if !array || matches!(ts[k].text.as_str(), "]" | "}") {
                            break;
                        }
                        k += 1;
                    }
                } else if matches!(key.as_str(), "message" | "prompt") && ts[j + 2].string {
                    accepted.push((ts[j + 2].start, ts[j + 2].end));
                }
            }
            j += 1;
        }
    }
    for id in map.threads.keys() {
        for (start, _) in code.match_indices(id) {
            if !accepted
                .iter()
                .any(|(a, b)| start >= *a && start + id.len() <= *b)
            {
                return Err(failure());
            }
        }
    }
    edits.sort_by_key(|(s, _, _)| *s);
    edits.dedup_by_key(|e| e.0);
    let mut result = code.to_owned();
    for (start, end, text) in edits.into_iter().rev() {
        result.replace_range(start..end, &text);
    }
    Ok(result)
}
fn remap(id: &mut Value, map: &IdentityMap) -> Result<(), TransferError> {
    if let Some(s) = id.as_str() {
        if let Some(new) = map.threads.get(s) {
            *id = new.clone().into();
        } else {
            return Err(failure());
        }
    }
    Ok(())
}
fn result(v: &mut Value, map: &IdentityMap) -> Result<bool, TransferError> {
    let mut recognized = false;
    if let Some(id) = v.get_mut("agent_id") {
        remap(id, map)?;
        recognized = true;
    }
    if let Some(status) = v.get_mut("status").and_then(Value::as_object_mut)
        && status.keys().all(|k| map.threads.contains_key(k))
    {
        let mut next = serde_json::Map::new();
        for (k, v) in std::mem::take(status) {
            next.insert(map.threads[&k].clone(), v);
        }
        *status = next;
        recognized = true;
    }
    Ok(recognized)
}
pub(super) fn output(value: &mut Value, map: &IdentityMap) -> Result<(), TransferError> {
    match value {
        Value::String(text) => {
            if !map.threads.keys().any(|id| text.contains(id)) {
                return Ok(());
            }
            let mut v: Value = serde_json::from_str(text).map_err(|_| failure())?;
            if !result(&mut v, map)? {
                return Err(failure());
            }
            *text = serde_json::to_string(&v)?;
        }
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.get_mut("text") {
                    output(text, map)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn rewrite(row: &mut Value, map: &IdentityMap) -> Result<(), TransferError> {
    if row["type"] != "response_item" {
        return Ok(());
    }
    let p = &mut row["payload"];
    if p["type"] == "custom_tool_call" && p["name"] == "exec" {
        if let Some(input) = p["input"].as_str() {
            p["input"] = script(input, map)?.into();
        }
    } else if p["type"] == "custom_tool_call_output"
        && p["call_id"]
            .as_str()
            .and_then(|id| map.tool_calls.get(id))
            .is_some_and(|name| name == "__code_agent")
    {
        output(&mut p["output"], map)?;
    }
    Ok(())
}

pub(super) fn references(code: &str) -> Vec<String> {
    let ts = tokens(code);
    let mut result = Vec::new();
    for i in 2..ts.len().saturating_sub(2) {
        if ts[i].string
            || ts[i - 1].text != "."
            || ts[i - 2].text != "tools"
            || ts[i + 1].text != "("
            || ts[i + 2].text != "{"
        {
            continue;
        }
        if !matches!(
            ts[i].text.as_str(),
            "multi_agent_v1__wait_agent"
                | "multi_agent_v1__send_input"
                | "multi_agent_v1__close_agent"
                | "multi_agent_v1__resume_agent"
        ) {
            continue;
        }
        let mut depth = 1i32;
        let mut j = i + 3;
        while j < ts.len() && depth > 0 {
            if !ts[j].string && ts[j].text == "{" {
                depth += 1;
            }
            if !ts[j].string && ts[j].text == "}" {
                depth -= 1;
            }
            if depth == 1
                && j + 2 < ts.len()
                && ts[j + 1].text == ":"
                && matches!(ts[j].text.as_str(), "target" | "targets" | "id" | "ids")
            {
                let mut k = j + 2;
                let array = ts[k].text == "[";
                if array {
                    k += 1;
                }
                while k < ts.len() {
                    if ts[k].string {
                        if uuid(&ts[k].text) {
                            result.push(ts[k].text.clone());
                        }
                        if !array {
                            break;
                        }
                    } else if (array && matches!(ts[k].text.as_str(), "]" | "}")) || !array {
                        break;
                    }
                    k += 1;
                }
            }
            j += 1;
        }
    }
    result
}
fn uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
pub(super) fn result_references(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .map(|v| {
                let mut ids = Vec::new();
                if let Some(id) = v["agent_id"].as_str()
                    && uuid(id)
                {
                    ids.push(id.into());
                }
                if let Some(status) = v["status"].as_object() {
                    ids.extend(status.keys().filter(|id| uuid(id)).cloned());
                }
                ids
            })
            .unwrap_or_default(),
        Value::Array(items) => items
            .iter()
            .flat_map(|v| result_references(&v["text"]))
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn agent_call(code: &str) -> bool {
    let ts = tokens(code);
    (2..ts.len().saturating_sub(1)).any(|i| {
        !ts[i].string
            && ts[i - 1].text == "."
            && ts[i - 2].text == "tools"
            && ts[i + 1].text == "("
            && matches!(
                ts[i].text.as_str(),
                "multi_agent_v1__wait_agent"
                    | "multi_agent_v1__send_input"
                    | "multi_agent_v1__close_agent"
                    | "multi_agent_v1__resume_agent"
                    | "multi_agent_v1__spawn_agent"
            )
    })
}
