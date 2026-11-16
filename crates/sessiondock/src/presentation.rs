//! Shared, viewport-independent conversation presentation. Plans refer to
//! message positions; original bodies and native identities stay authoritative.
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn role(m: &Value) -> &str {
    m["role"].as_str().unwrap_or("")
}
fn assistant(m: &Value) -> bool {
    role(m).split('·').next() == Some("assistant")
}
fn final_answer(m: &Value) -> bool {
    assistant(m)
        && matches!(
            m["phase"].as_str(),
            Some("final" | "final_answer" | "end_turn")
        )
}
fn user(m: &Value) -> bool {
    matches!(role(m), "user" | "user·subagent")
}
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        _ => v.to_string(),
    }
}
fn message_text(m: &Value) -> String {
    text(if m["text"].is_null() {
        &m["content"]
    } else {
        &m["text"]
    })
}
fn present(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.),
        _ => true,
    }
}
fn nonempty(v: &Value) -> bool {
    v.as_array().is_some_and(|a| !a.is_empty())
}
fn same_turn(a: &Value, b: &Value) -> bool {
    !a["turn_id"].is_null() && !b["turn_id"].is_null() && text(&a["turn_id"]) == text(&b["turn_id"])
}
fn tool(m: &Value) -> bool {
    matches!(role(m), "tool" | "tool_result") && !nonempty(&m["changes"])
}

#[derive(Clone)]
struct Entry {
    index: usize,
    result: Option<usize>,
    silent: bool,
}
impl Entry {
    fn wire(&self) -> Value {
        json!({"index":self.index,"result":self.result,"silent":self.silent})
    }
}
struct Planner<'a> {
    messages: &'a [&'a Value],
}
impl Planner<'_> {
    fn messages(&self, ids: &[usize], open_tail: bool) -> Vec<Value> {
        let mut entries: Vec<Entry> = Vec::new();
        let mut calls = BTreeMap::new();
        for &i in ids {
            let m = self.messages[i];
            if role(m) == "tool" {
                if present(&m["call_id"]) {
                    calls.insert(text(&m["call_id"]), entries.len());
                }
                entries.push(Entry {
                    index: i,
                    result: None,
                    silent: false,
                });
            } else if role(m) == "tool_result"
                && present(&m["call_id"])
                && let Some(owner) = calls.remove(&text(&m["call_id"]))
            {
                entries[owner].result = Some(i);
            } else {
                entries.push(Entry {
                    index: i,
                    result: None,
                    silent: false,
                });
            }
        }
        let mut out = Vec::new();
        let mut run = Vec::new();
        let flush = |out: &mut Vec<Value>, run: &mut Vec<Value>, tail: bool| {
            if !run.is_empty() {
                out.push(json!({"g":std::mem::take(run),"tail":tail}));
            }
        };
        for mut entry in entries {
            let m = self.messages[entry.index];
            if role(m) == "tool_result"
                && message_text(m).trim().is_empty()
                && !nonempty(&m["media"])
                && !nonempty(&m["native_files"])
                && !nonempty(&m["changes"])
            {
                flush(&mut out, &mut run, false);
                entry.silent = true;
                out.push(json!({"m":entry.wire()}));
            } else if tool(m) {
                run.push(entry.wire());
            } else {
                flush(&mut out, &mut run, false);
                out.push(json!({"m":entry.wire()}));
            }
        }
        flush(&mut out, &mut run, open_tail);
        out
    }
    fn key(&self, ids: &[usize], native: bool) -> String {
        if native && let Some(&first) = ids.first() {
            let m = self.messages[first];
            return if present(&m["turn_id"]) {
                text(&m["turn_id"])
            } else {
                text(&m["id"])
            };
        }
        if let Some(m) = ids
            .iter()
            .map(|&i| self.messages[i])
            .find(|m| present(&m["turn_id"]))
        {
            return text(&m["turn_id"]);
        }
        let Some(&first) = ids.first() else {
            return "turn:".into();
        };
        let m = self.messages[first];
        if native {
            return text(&m["id"]);
        }
        format!(
            "{}:{}",
            m["ts"].as_str().filter(|s| !s.is_empty()).unwrap_or("turn"),
            message_text(m).chars().take(80).collect::<String>()
        )
    }
    fn summary(&self, ids: &[usize], native: bool) -> Value {
        let mut counts = [0usize; 5];
        let mut errors = 0;
        let mut paths = Vec::<String>::new();
        let mut added = 0.;
        let mut removed = 0.;
        let mut times = Vec::new();
        let number = |v: &Value| -> Option<f64> {
            match v {
                Value::Null => Some(0.),
                Value::Bool(b) => Some(if *b { 1. } else { 0. }),
                Value::Number(n) => n.as_f64(),
                Value::String(s) => {
                    if s.trim().is_empty() {
                        Some(0.)
                    } else {
                        s.parse().ok()
                    }
                }
                _ => None,
            }
        };
        for &i in ids {
            let m = self.messages[i];
            if assistant(m) {
                counts[0] += 1;
            }
            match role(m) {
                "thinking" => counts[1] += 1,
                "tool" => counts[2] += 1,
                "tool_result" => counts[3] += 1,
                "question" => counts[4] += 1,
                _ => {}
            }
            let result_error = if native && role(m) == "tool" {
                let result = if m["result"].is_null() {
                    &m["tool_result"]
                } else {
                    &m["result"]
                };
                let failed = present(&m["error"])
                    || present(&m["is_error"])
                    || matches!(m["status"].as_str(), Some("failed" | "ERROR"));
                if result["role"] == "tool_result" {
                    present(&result["error"])
                } else {
                    failed
                        && (!result.is_null()
                            || present(&m["tool_content"])
                            || matches!(m["status"].as_str(), Some("failed" | "ERROR")))
                }
            } else {
                present(&m["result"]["error"])
            };
            if present(&m["error"])
                || result_error
                || number(&m["exit_code"]).is_some_and(|n| n.is_finite() && n != 0.)
            {
                errors += 1;
            }
            if let Some(changes) = m["changes"].as_array() {
                for change in changes {
                    if present(&change["path"]) {
                        let path = text(&change["path"]);
                        if !paths.contains(&path) {
                            paths.push(path);
                        }
                    }
                    added += number(&change["added"])
                        .filter(|n| n.is_finite())
                        .unwrap_or(0.);
                    removed += number(&change["removed"])
                        .filter(|n| n.is_finite())
                        .unwrap_or(0.);
                }
            }
            if let Some(at) = m["ts"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            {
                times.push(at.timestamp_millis());
            }
        }
        let mut stats = Vec::new();
        if counts[0] > 0 {
            stats.push(format!("{} 条进展", counts[0]));
        }
        if counts[1] > 0 {
            stats.push(format!("{} 段思考", counts[1]));
        }
        let calls = if counts[2] > 0 { counts[2] } else { counts[3] };
        if calls > 0 {
            stats.push(format!("🔧 {calls}"));
        }
        if !paths.is_empty() {
            stats.push(format!(
                "修改 {} 个文件{}",
                paths.len(),
                if added != 0. || removed != 0. {
                    format!(" +{added} −{removed}")
                } else {
                    String::new()
                }
            ));
        }
        if counts[4] > 0 {
            stats.push(format!("{} 次确认", counts[4]));
        }
        if errors > 0 {
            stats.push(format!("调用报错 {errors}"));
        }
        if times.len() > 1 {
            let duration = times.iter().max().unwrap() - times.iter().min().unwrap();
            if duration >= 1000 {
                let seconds = duration as f64 / 1000.;
                if seconds < 10. {
                    stats.push(format!("{} 秒", seconds.round()));
                } else {
                    let seconds = seconds.round() as i64;
                    let (h, m, s) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
                    let mut parts = Vec::new();
                    if h > 0 {
                        parts.push(format!("{h} 小时"));
                    }
                    if m > 0 {
                        parts.push(format!("{m} 分"));
                    }
                    if s > 0 || (h == 0 && m == 0) {
                        parts.push(format!("{s} 秒"));
                    }
                    stats.push(parts.join(" "));
                }
            }
        }
        if stats.is_empty() {
            stats.push(format!("{} 条记录", ids.len()));
        }
        json!({"stats":stats,"paths":paths,"errors":errors})
    }
    fn segment(
        &self,
        ids: &[usize],
        prompt: usize,
        complete: bool,
        foldable: bool,
        open_tail: bool,
        native: bool,
    ) -> Vec<Value> {
        if let Some((&last, prefix)) = ids.split_last()
            && role(self.messages[last]) == "event"
            && present(&self.messages[last]["subtask_outcome"])
        {
            let mut out = self.segment(prefix, prompt.min(prefix.len()), true, true, false, native);
            out.push(json!({"m":{"index":last}}));
            return out;
        }
        let body = &ids[prompt..];
        let meaningful = body
            .iter()
            .rposition(|&i| self.messages[i]["counted"] != false)
            .map_or(0, |i| i + 1);
        let interrupted = ids
            .iter()
            .any(|&i| present(&self.messages[i]["interrupted"]));
        // (start, end, tail start, inferred, interrupted, continued)
        let mut conclusion: Option<(usize, usize, usize, bool, bool, bool)> = None;
        for i in 0..meaningful {
            let m = self.messages[body[i]];
            if role(m) == "assistant" && final_answer(m) {
                let mut end = i + 1;
                while end < meaningful && final_answer(self.messages[body[end]]) {
                    end += 1;
                }
                conclusion = Some((i, end, end, false, false, end < meaningful));
                break;
            }
        }
        if conclusion.is_none() && meaningful > 0 {
            let last = self.messages[body[meaningful - 1]];
            if final_answer(last) {
                let mut start = meaningful - 1;
                while start > 0 && final_answer(self.messages[body[start - 1]]) {
                    start -= 1;
                }
                conclusion = Some((start, meaningful, meaningful, false, false, false));
            } else if complete && interrupted {
                if let Some(i) = (0..meaningful).rev().find(|&i| {
                    assistant(self.messages[body[i]])
                        && present(&self.messages[body[i]]["interrupted"])
                }) {
                    conclusion = Some((i, i + 1, meaningful, true, true, false));
                }
            } else if complete && assistant(last) && !present(&last["phase"]) {
                conclusion = Some((meaningful - 1, meaningful, meaningful, true, false, false));
            }
        }
        if conclusion.is_none() && !foldable {
            return self.messages(ids, open_tail);
        }
        let (process, final_ids, rest, inferred, stopped, continued) = match conclusion {
            Some((start, end, tail, inferred, stopped, continued)) => {
                let mut process = body[..start].to_vec();
                if stopped {
                    process.extend_from_slice(&body[end..tail]);
                }
                (
                    process,
                    body[start..end].to_vec(),
                    body[tail..].to_vec(),
                    inferred,
                    stopped,
                    continued,
                )
            }
            None => (body.to_vec(), Vec::new(), Vec::new(), false, false, false),
        };
        let plan = self.messages(&process, false);
        let size = plan
            .iter()
            .filter(|p| {
                p.get("g").is_some()
                    || p.get("m").is_some_and(|m| {
                        m["silent"] != true
                            && self.messages[m["index"].as_u64().unwrap() as usize]["silent"]
                                != true
                    })
            })
            .count();
        let lone_thinking = size == 1
            && plan.iter().any(|p| {
                p["m"]["index"]
                    .as_u64()
                    .is_some_and(|i| role(self.messages[i as usize]) == "thinking")
            });
        let tail = if continued {
            self.segment(&rest, 0, complete, foldable, open_tail, native)
        } else {
            self.messages(&rest, false)
        };
        if size == 0 || (size < 2 && !stopped && !lone_thinking) {
            if !continued {
                return self.messages(ids, false);
            }
            let end = conclusion.unwrap().1;
            let mut out = self.messages(&ids[..prompt + end], false);
            out.extend(tail);
            return out;
        }
        let mut out: Vec<Value> = ids[..prompt]
            .iter()
            .enumerate()
            .map(|(i, id)| json!({"m":{"index":id},"sealedTurnHead":i==0}))
            .collect();
        out.push(json!({"turn":{"items":process,"plan":plan,"key":self.key(ids,native),
            "summary":self.summary(&process, native),"hasConclusion":!final_ids.is_empty(),"inferred":inferred,"interrupted":stopped}}));
        out.extend(self.messages(&final_ids, false));
        out.extend(tail);
        out
    }
    fn turns(&self, ids: &[usize], complete: bool, fold_tail: bool, native: bool) -> Vec<Value> {
        let Some(mut start) = ids.iter().position(|&i| user(self.messages[i])) else {
            return self.segment(ids, 0, complete, fold_tail, true, native);
        };
        let mut out = self.segment(&ids[..start], 0, true, true, false, native);
        while start < ids.len() {
            let mut next = start + 1;
            while next < ids.len()
                && user(self.messages[ids[next]])
                && same_turn(self.messages[ids[start]], self.messages[ids[next]])
            {
                next += 1;
            }
            while next < ids.len() && !user(self.messages[ids[next]]) {
                next += 1;
            }
            let turn = &ids[start..next];
            let mut prompt = 1;
            while prompt < turn.len()
                && ((user(self.messages[turn[prompt]])
                    && same_turn(self.messages[turn[0]], self.messages[turn[prompt]]))
                    || (role(self.messages[turn[prompt]]) == "system"
                        && self.messages[turn[prompt]]["native_type"] == "USER_SETTINGS_CHANGE"))
            {
                prompt += 1;
            }
            let historical = next < ids.len();
            out.extend(self.segment(
                turn,
                prompt,
                historical || complete,
                historical || fold_tail,
                !historical,
                native,
            ));
            start = next;
        }
        out
    }
}

/// Head and tail are separate spans: no process or conclusion crosses a gap.
pub(crate) fn plans(messages: &[&Value], split: Option<usize>, native: bool) -> Value {
    let planner = Planner { messages };
    let ranges = match split {
        Some(n) => vec![
            (0, n.min(messages.len())),
            (n.min(messages.len()), messages.len()),
        ],
        None => vec![(0, messages.len())],
    };
    let spans: Vec<Value>=ranges.into_iter().filter(|(a,b)|a<b).map(|(start,end)| {
        let ids: Vec<_>=(start..end).collect();
        json!({"start":start,"end":end,"active":planner.turns(&ids,false,false,native),
            "fold":planner.turns(&ids,false,true,native),"complete":planner.turns(&ids,true,true,native)})
    }).collect();
    json!({"version":1,"spans":spans})
}

/// Runs at the provider's publication point, before fan-out to browser streams.
pub(crate) fn native_snapshot(source: &str, mut snapshot: Value) -> Value {
    let Some(items) = snapshot["raw"]["items"].as_array() else {
        return snapshot;
    };
    let messages: Vec<_> = items
        .iter()
        .filter(|m| {
            !(source == "kiro"
                && role(m) == "system"
                && (m["kind"] == "compact" || m["type"] == "compact"))
                && !(source == "agy" && role(m) == "system" && m["native_type"] == "checkpoint")
        })
        .collect();
    let complete = snapshot["raw"]["running"] != true && !nonempty(&snapshot["raw"]["requests"]);
    let ids: Vec<_> = (0..messages.len()).collect();
    let planner = Planner {
        messages: &messages,
    };
    let mut span = json!({"start": 0, "end": messages.len()});
    span[if complete { "complete" } else { "active" }] =
        json!(planner.turns(&ids, complete, complete, true));
    let mut presentation = json!({"version": 1, "spans": [span]});
    presentation["ids"] = json!(messages.iter().map(|m| text(&m["id"])).collect::<Vec<_>>());
    snapshot["raw"]["presentation"] = presentation;
    snapshot
}
