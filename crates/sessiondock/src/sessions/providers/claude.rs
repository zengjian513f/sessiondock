//! Claude's append-only transcript is a tree, not a flat event log.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::{
    ParseOptions, Parser, Skipped, first_nonempty, normalized, string, timeline_protocol, truthy,
};

pub(super) struct Lineage {
    pub active: Option<HashSet<String>>,
    /// User inputs off the active lineage that stay
    /// visible as interrupted — unanswered siblings of the current input, the
    /// input whose turn an Esc cut short (even with tools/thinking under it)
    /// and unanswered inputs below those.
    pub abandoned: HashSet<String>,
    /// Every descendant of an abandoned input that is
    /// neither active nor abandoned (the tool calls the interrupted assistant
    /// wrote, the native interrupt record). Visible, never a turn start.
    pub offshoot: HashSet<String>,
    /// Abandoned inputs with a native interrupt record or a response below
    /// them: their `aborted` status comes from that record, not right after
    /// the input.
    pub deferred_abort: HashSet<String>,
    pub tip: Option<String>,
    /// Graph node → parent after compact reconnection; pin validation only.
    pub parents: HashMap<String, Option<String>>,
    /// Non-fatal `migration_warnings` of the walk: where it
    /// stopped short of a root.
    pub warnings: Vec<String>,
}

/// Effective parse options derived from a persisted display pin, plus the
/// explicit reason when native records appended after the pin retire it.
pub(in crate::sessions) struct PinOutcome {
    pub declared_tip: Option<String>,
    pub abandoned_after: u64,
    pub retired: Option<PinRetirement>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::sessions) enum PinRetirement {
    /// A native signal after the pin names exactly the pinned tip.
    NativeConfirmed,
    /// The first native node after the pin descends directly from the tip.
    NativeContinued,
    /// New native nodes descend from the tip but not directly: the CLI never
    /// rewound and kept going past the pinned point.
    NativeAdvanced,
    /// The new native leaf does not descend from the pinned tip at all.
    NativeDiverged,
    /// The pinned tip is no longer a node of the current record set.
    TipMissing,
}

impl PinRetirement {
    pub fn code(self) -> &'static str {
        match self {
            Self::NativeConfirmed => "native_confirmed",
            Self::NativeContinued => "native_continued",
            Self::NativeAdvanced => "native_advanced",
            Self::NativeDiverged => "native_diverged",
            Self::TipMissing => "tip_missing",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::NativeConfirmed => "原生记录已确认固定的时间线",
            Self::NativeContinued => "CLI 已从固定点继续，原生记录重新接管显示",
            Self::NativeAdvanced => "CLI 未回滚，已在固定点之后继续；固定显示已失效",
            Self::NativeDiverged => "CLI 已切换到其他分支；固定显示已失效",
            Self::TipMissing => "固定的叶子已不在原生记录中；固定显示已失效",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::sessions) enum PinTargetError {
    /// Not a graph node of this session at all.
    Unknown,
    /// A node, but not on the current active lineage.
    Inactive,
    /// The first node has nothing before it to display.
    NoParent,
    Unsupported(String),
}

/// Graph nodes with the same compact reconnection as `lineage`.
struct Graph {
    parents: HashMap<String, Option<String>>,
    /// Parent of each graph node in file order, with the record's end offset.
    ordered: Vec<(Option<String>, u64)>,
}

fn graph(records: &[(Value, u64)], agent: &str) -> Graph {
    let mut parents = HashMap::<String, Option<String>>::new();
    let mut ordered = Vec::new();
    let mut scan_tip: Option<String> = None;
    for (record, end) in records {
        if let Some(id) = graph_id(record, agent) {
            let parent = record["parentUuid"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned);
            let parent = if parent.is_none() && compact(record) {
                scan_tip.clone()
            } else {
                parent
            };
            parents.insert(id.to_owned(), parent.clone());
            ordered.push((parent, *end));
        }
        if let Some(next) = signal(record, agent) {
            scan_tip = Some(next.to_owned());
        }
    }
    Graph { parents, ordered }
}

fn chain_contains(parents: &HashMap<String, Option<String>>, from: &str, wanted: &str) -> bool {
    let mut seen = HashSet::new();
    let mut node = Some(from);
    while let Some(id) = node {
        if id == wanted {
            return true;
        }
        if !seen.insert(id) {
            return false;
        }
        node = parents.get(id).and_then(Option::as_deref);
    }
    false
}

/// Apply a persisted pin to the main-session records.
/// Any lineage signal recorded after the pinned
/// boundary makes native records authoritative again; the
/// supersession is reported explicitly instead of silently. Pure: no writes.
pub(super) fn apply_pin(records: &[(Value, u64)], tip: &str, stale_end: u64) -> PinOutcome {
    let Graph { parents, ordered } = graph(records, "");
    if !parents.contains_key(tip) {
        return PinOutcome {
            declared_tip: None,
            abandoned_after: 0,
            retired: Some(PinRetirement::TipMissing),
        };
    }
    let mut appended: Option<&str> = None;
    for (record, end) in records {
        if *end > stale_end
            && let Some(next) = signal(record, "")
        {
            appended = Some(next);
        }
    }
    let Some(appended) = appended else {
        return PinOutcome {
            declared_tip: Some(tip.to_owned()),
            abandoned_after: stale_end,
            retired: None,
        };
    };
    let first_new_parent = ordered
        .iter()
        .find(|(_, end)| *end > stale_end)
        .map(|(parent, _)| parent.as_deref());
    let retired = if appended == tip {
        PinRetirement::NativeConfirmed
    } else if chain_contains(&parents, appended, tip) {
        if first_new_parent == Some(Some(tip)) {
            PinRetirement::NativeContinued
        } else {
            PinRetirement::NativeAdvanced
        }
    } else {
        PinRetirement::NativeDiverged
    };
    PinOutcome {
        declared_tip: None,
        abandoned_after: stale_end,
        retired: Some(retired),
    }
}

/// Resolve an operator-chosen node into the tip displayed after "rewinding to
/// before it", exactly like Claude's native selector keeps everything before
/// the chosen prompt. Validates against the natural (unpinned) active lineage.
pub(super) fn resolve_pin_target(
    records: &[(Value, u64)],
    target: &str,
) -> Result<String, PinTargetError> {
    let lineage = lineage(records, ParseOptions::default());
    let Some(active) = lineage.active else {
        return Err(PinTargetError::Unsupported(
            "没有可固定的 Claude 时间线".to_owned(),
        ));
    };
    let Some(parent) = lineage.parents.get(target) else {
        return Err(PinTargetError::Unknown);
    };
    if !active.contains(target) {
        return Err(PinTargetError::Inactive);
    }
    parent.clone().ok_or(PinTargetError::NoParent)
}

pub(super) fn compact(record: &Value) -> bool {
    record["type"] == "system"
        && (record["subtype"] == "compact_boundary" || record["compactMetadata"].is_object())
}

pub(super) fn graph_id<'a>(record: &'a Value, agent: &str) -> Option<&'a str> {
    if (agent.is_empty() && truthy(&record["isSidechain"])) || record.get("parentUuid").is_none() {
        return None;
    }
    record["uuid"].as_str().filter(|id| !id.is_empty())
}

fn signal<'a>(record: &'a Value, agent: &str) -> Option<&'a str> {
    if record["type"] == "last-prompt" {
        record["leafUuid"]
            .as_str()
            .filter(|id| !id.is_empty())
            .or_else(|| graph_id(record, agent))
    } else {
        graph_id(record, agent)
    }
}

/// Lineage over the main-session (or one agent's) records.
/// A broken chain is a warning, never a failure.
pub(super) fn lineage(records: &[(Value, u64)], options: ParseOptions<'_>) -> Lineage {
    let mut parents = HashMap::<String, Option<String>>::new();
    let mut users = HashMap::<String, (Option<String>, u64)>::new();
    let mut responses = HashSet::<String>::new();
    let mut interrupts = Vec::<String>::new();
    let mut scan_tip = None;
    let declared = options.declared_tip.filter(|tip| !tip.is_empty());
    let mut tip = declared.map(str::to_owned);
    for (record, end) in records {
        if let Some(id) = graph_id(record, options.agent) {
            let parent = record["parentUuid"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned);
            let parent = if parent.is_none() && compact(record) {
                scan_tip.clone()
            } else {
                parent
            };
            parents.insert(id.to_owned(), parent.clone());
            if options.agent.is_empty() && record["type"] == "user" {
                users.insert(id.to_owned(), (parent, *end));
                if interrupt_record(record) {
                    interrupts.push(id.to_owned());
                }
            } else if (record["type"] == "assistant" && truthy(&record["message"]["content"]))
                || (record["type"] == "system" && record["subtype"] == "turn_duration")
            {
                responses.insert(id.to_owned());
            }
        }
        if let Some(next) = signal(record, options.agent) {
            scan_tip = Some(next.to_owned());
            if declared.is_none() {
                tip = scan_tip.clone()
            }
        }
    }
    let Some(tip_id) = tip.as_ref() else {
        return Lineage {
            active: None,
            abandoned: HashSet::new(),
            offshoot: HashSet::new(),
            deferred_abort: HashSet::new(),
            tip,
            parents,
            warnings: Vec::new(),
        };
    };
    // The walk stops at a uuid without a record or
    // one already visited; the reachable part is the timeline. The missing
    // uuid itself is in `active` (it never matches a record), so a declared
    // leaf with no record leaves every graph node out.
    let mut active = HashSet::<String>::new();
    let mut warnings = Vec::new();
    let mut node = Some(tip_id.as_str());
    while let Some(id) = node {
        if !active.insert(id.to_owned()) {
            warnings.push(format!("Claude 祖先链存在循环，已在 {id} 处截断"));
            break;
        }
        let Some(parent) = parents.get(id) else {
            warnings.push(if id == tip_id {
                format!("Claude 声明的叶子 {id} 不在记录中")
            } else {
                format!("Claude 祖先链在 {id} 处中断，之前的记录不在当前时间线")
            });
            break;
        };
        node = parent.as_deref();
    }
    let mut responded = HashSet::<String>::new();
    for response in &responses {
        let mut node = parents.get(response).and_then(Option::as_deref);
        while let Some(id) = node {
            if !responded.insert(id.to_owned()) {
                break;
            }
            node = parents.get(id).and_then(Option::as_deref);
        }
    }
    let active_user_parents = users
        .iter()
        .filter(|(id, _)| active.contains(*id))
        .filter_map(|(_, (parent, _))| parent.as_deref())
        .collect::<HashSet<_>>();
    // An unanswered input whose sibling is the current input: a fast Esc
    // committed the row, the next input went beside it.
    let abandoned_sibling = |id: &str, parent: Option<&str>, end: u64| {
        !active.contains(id)
            && parent.is_some_and(|parent| {
                active.contains(parent) && active_user_parents.contains(parent)
            })
            && end > options.abandoned_after
    };
    let mut abandoned = users
        .iter()
        .filter(|(id, (parent, end))| {
            !responded.contains(*id) && abandoned_sibling(id, parent.as_deref(), *end)
        })
        .map(|(id, _)| id.clone())
        .collect::<HashSet<_>>();
    // An Esc after the assistant already wrote tools/thinking: the next input
    // hangs off the previous turn_duration, so the interrupted turn looks like
    // a completed old branch. Walk up from the interrupt record to the input
    // it cut short; unlike the sibling rule, having a reply does not hide it.
    for interrupt in &interrupts {
        let mut seen = HashSet::new();
        let mut node = Some(interrupt.as_str());
        while let Some(id) = node {
            if !seen.insert(id) {
                break;
            }
            let parent = parents.get(id).and_then(Option::as_deref);
            if let Some((_, end)) = users.get(id)
                && abandoned_sibling(id, parent, *end)
            {
                abandoned.insert(id.to_owned());
                break;
            }
            node = parent;
        }
    }
    let mut children = HashMap::<&str, Vec<&str>>::new();
    for (id, parent) in &parents {
        if let Some(parent) = parent {
            children
                .entry(parent.as_str())
                .or_default()
                .push(id.as_str());
        }
    }
    // Everything below an abandoned input stays visible; unanswered inputs
    // among it are abandoned inputs too.
    let mut offshoot = HashSet::<String>::new();
    let mut stack = abandoned.iter().cloned().collect::<Vec<_>>();
    while let Some(node) = stack.pop() {
        for child in children.get(node.as_str()).into_iter().flatten() {
            if active.contains(*child) || abandoned.contains(*child) || offshoot.contains(*child) {
                continue;
            }
            offshoot.insert((*child).to_owned());
            stack.push((*child).to_owned());
        }
    }
    for id in &offshoot {
        if users.contains_key(id) && !responded.contains(id) {
            abandoned.insert(id.clone());
        }
    }
    let mut deferred_abort = HashSet::<String>::new();
    for id in &abandoned {
        let mut seen = HashSet::new();
        let mut stack = children.get(id.as_str()).cloned().unwrap_or_default();
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            if interrupts.iter().any(|interrupt| interrupt == node) || responses.contains(node) {
                deferred_abort.insert(id.clone());
                break;
            }
            stack.extend(children.get(node).into_iter().flatten().copied());
        }
    }
    Lineage {
        active: Some(active),
        abandoned,
        offshoot,
        deferred_abort,
        tip,
        parents,
        warnings,
    }
}

/// The native mark of an Esc, not a
/// new input. Texts are the string content or the string / `text` blocks.
fn interrupt_record(record: &Value) -> bool {
    if record["type"] != "user" {
        return false;
    }
    if truthy(&record["interruptedMessageId"]) {
        return true;
    }
    match &record["message"]["content"] {
        Value::String(text) => interrupt_text(text),
        Value::Array(items) => items.iter().any(|item| match item {
            Value::String(text) => interrupt_text(text),
            Value::Object(_) if item["type"] == "text" => interrupt_text(&string(&item["text"])),
            _ => false,
        }),
        _ => false,
    }
}

/// Whole-text, case-insensitive.
fn interrupt_text(text: &str) -> bool {
    matches!(
        text.trim().to_ascii_lowercase().as_str(),
        "[request interrupted by user]" | "[request interrupted by user for tool use]"
    )
}

/// What the lineage decided about one visible record off the active chain.
#[derive(Clone, Copy, Default)]
pub(super) struct Branch {
    /// An abandoned input — no `working`
    /// status, `interrupted:true` on its bubbles.
    pub abandoned: bool,
    /// The `aborted` status is emitted by the
    /// native interrupt record (or ends with the response) below it.
    pub deferred_abort: bool,
}

/// Content blocks of one record. Array elements that are neither strings nor
/// objects are skipped and counted like the reference `_flatten_content`;
/// a scalar `content` stays a hard failure (the reference adapter raises).
fn blocks<'a>(
    content: &'a Value,
    skipped: &mut Skipped,
) -> Result<Vec<std::borrow::Cow<'a, Value>>, String> {
    use std::borrow::Cow::{Borrowed, Owned};
    match content {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(vec![Owned(json!({"type": "text", "text": text}))]),
        Value::Object(_) => Ok(vec![Borrowed(content)]),
        Value::Array(items) => Ok(items
            .iter()
            .filter_map(|item| {
                if item.is_string() {
                    Some(Owned(json!({"type": "text", "text": item})))
                } else if item.is_object() {
                    Some(Borrowed(item))
                } else {
                    skipped.note("内容块类型", "(非对象元素)");
                    None
                }
            })
            .collect()),
        _ => Err("Claude content 不是字符串、对象或数组".to_owned()),
    }
}

fn part_text(part: &Value) -> Option<&str> {
    matches!(
        part["type"].as_str(),
        Some("text" | "input_text" | "output_text" | "summary_text")
    )
    .then(|| part["text"].as_str())
    .flatten()
}

fn clear_turn(parser: &mut Parser) {
    if let Some(event) = parser.events.last_mut() {
        event
            .message
            .as_object_mut()
            .expect("message object")
            .remove("turn_id");
    }
}

pub(super) fn record(
    parser: &mut Parser,
    record: &Value,
    end: u64,
    agent: &str,
    branch: Branch,
) -> Result<(), String> {
    let abandoned = branch.abandoned;
    let kind = record["type"].as_str().unwrap_or("");
    let ts = normalized(&record["timestamp"]);
    let tag = if !agent.is_empty() {
        Some(agent.chars().take(8).collect::<String>())
    } else if truthy(&record["isSidechain"]) {
        record["agentId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(|id| id.chars().take(8).collect())
    } else {
        None
    };
    match kind {
        "user" | "assistant" => {
            if truthy(&record["isMeta"]) || truthy(&record["isCompactSummary"]) {
                return Ok(());
            }
            let parts = blocks(&record["message"]["content"], &mut parser.skipped)?;
            let visible_user = parts.iter().any(|part| {
                super::image_content::image_shape(part)
                    || parser
                        .media
                        .as_ref()
                        .and_then(|context| context.lookup(part))
                        .is_some()
            }) || parts.iter().filter_map(|part| part_text(part)).any(|text| {
                (kind == "user" && tag.is_none() && bash_input(text).is_some())
                    || (!(kind == "user" && tag.is_none() && bash_output(text).is_some())
                        && !text.trim().is_empty()
                        && text.trim() != "/compact"
                        && notification(text).is_none()
                        && !timeline_protocol(text))
            });
            let is_interrupt = kind == "user"
                && tag.is_none()
                && (truthy(&record["interruptedMessageId"])
                    || parts
                        .iter()
                        .filter_map(|part| part_text(part))
                        .any(interrupt_text));
            // A real input is the structural start of a turn, an abandoned
            // one included; the native interrupt mark ends the current turn.
            if kind == "user"
                && (tag.is_none() || !agent.is_empty())
                && !is_interrupt
                && visible_user
            {
                parser.turn = first_nonempty(&[record["uuid"].clone()], &end.to_string());
            }
            if is_interrupt {
                parser.status(end, "aborted", &ts, json!({}));
                return Ok(());
            }
            if kind == "user" && tag.is_none() && !abandoned && visible_user {
                parser.status(end, "working", &ts, json!({}));
            }
            let role = if tag.is_some() {
                format!("{kind}·subagent")
            } else {
                kind.to_owned()
            };
            let phase = if kind == "assistant" {
                match record["message"]["stop_reason"].as_str() {
                    Some("end_turn") => Some("final"),
                    Some(value) if !value.is_empty() => Some("progress"),
                    _ => None,
                }
            } else {
                None
            };
            let mut emitted_abandoned = false;
            for part in parts {
                let before = parser.events.len();
                if let Some(text) = part_text(&part) {
                    if text.trim().is_empty() {
                        continue;
                    }
                    if role == "user"
                        && tag.is_none()
                        && let Some(command) = bash_input(text)
                    {
                        let event_id = first_nonempty(&[record["uuid"].clone()], &end.to_string());
                        let call = format!("local-shell:{event_id}");
                        parser.emit(
                            end,
                            "command",
                            command,
                            &ts,
                            json!({"call_id": call, "local_shell": true, "event_id": call}),
                        );
                        emitted_abandoned |= abandoned;
                    } else if role == "user"
                        && tag.is_none()
                        && let Some((text, stderr)) = bash_output(text)
                    {
                        let parent = first_nonempty(
                            &[record["parentUuid"].clone(), record["uuid"].clone()],
                            &end.to_string(),
                        );
                        parser.emit(
                            end,
                            "tool_result",
                            text,
                            &ts,
                            json!({
                                "name": "Shell", "call_id": format!("local-shell:{parent}"),
                                "counted": false, "has_stderr": stderr,
                            }),
                        );
                    } else if tag.is_none()
                        && let Some((summary, status, details)) = notification(text)
                    {
                        parser.emit(
                            end,
                            "event",
                            summary,
                            &ts,
                            json!({
                                "event_kind": "task", "event_status": status,
                                "details": details, "counted": false,
                            }),
                        );
                        clear_turn(parser);
                    } else if role.starts_with("user")
                        && (timeline_protocol(text) || (tag.is_none() && text.trim() == "/compact"))
                    {
                        continue;
                    } else {
                        parser.content(end, &role, &part, &ts, phase)?;
                        emitted_abandoned |= abandoned;
                    }
                } else {
                    parser.content(end, &role, &part, &ts, phase)?;
                    emitted_abandoned |= abandoned
                        && parser.events[before..]
                            .iter()
                            .any(|event| !event.media.is_empty());
                }
                if tag.is_some() {
                    let mut added = parser.events.split_off(before);
                    added.retain(|event| event.message["role"] != "status");
                    for event in &mut added {
                        if event.message["role"] == role || event.message["role"] == "thinking" {
                            event.message["name"] = json!(tag);
                        }
                    }
                    parser.events.extend(added);
                }
                if abandoned {
                    for event in &mut parser.events[before..] {
                        if event.message["role"] == "user"
                            || (event.message["role"] == "command"
                                && event.message["local_shell"] == true)
                        {
                            event.message["interrupted"] = json!(true);
                            event.message["interrupt_reason"] =
                                json!("输入已中断，未进入当前 Claude 分支");
                        }
                    }
                }
            }
            if emitted_abandoned && !branch.deferred_abort {
                parser.status(
                    end,
                    "aborted",
                    &ts,
                    json!({"reason": "输入已中断，未进入当前 Claude 分支"}),
                );
            }
        }
        "system" => {
            if tag.is_none() && record["subtype"] == "turn_duration" {
                parser.status(
                    end,
                    "idle",
                    &ts,
                    json!({"duration_ms": record["durationMs"]}),
                );
                if record["durationMs"]
                    .as_f64()
                    .is_some_and(|duration| duration >= 0.0)
                {
                    parser.emit(end, "event", "", &ts, json!({
                        "counted": false, "event_kind": "duration", "duration_ms": record["durationMs"],
                    }));
                }
            } else if tag.is_none()
                && record["subtype"] == "away_summary"
                && truthy(&record["content"])
            {
                parser.emit(
                    end,
                    "event",
                    string(&record["content"]),
                    &ts,
                    json!({"counted": false, "event_kind": "recap"}),
                );
            } else if record["subtype"] == "local_command" {
                if tag.is_none()
                    && let Some(command) = local_command(&string(&record["content"]))
                {
                    let verb = command.split_whitespace().next().unwrap_or("");
                    if !["/rename", "/compact"].contains(&verb) {
                        let id = first_nonempty(&[record["uuid"].clone()], &end.to_string());
                        parser.emit(end, "command", command, &ts,
                                    json!({"counted": false, "inferred": true, "event_id": format!("command:{id}")}));
                    }
                }
            } else if compact(record) {
                if tag.is_none() {
                    parser.status(end, "idle", &ts, json!({}))
                }
                let id = first_nonempty(&[record["uuid"].clone(), ts.clone()], &end.to_string());
                parser.emit(end, "event", "已压缩", &ts,
                            json!({"counted": false, "event_kind": "compact", "event_id": format!("compact:{id}")}));
            } else if truthy(&record["content"]) {
                parser.emit(end, "system", string(&record["content"]), &ts, json!({}));
            }
        }
        "attachment" => {
            let attachment = &record["attachment"];
            let kind = attachment["type"].as_str().unwrap_or("");
            // Only a queued human prompt renders. Every other attachment kind
            // (environment, hook_success, model, diagnostics, …) is skipped
            // like the reference adapter; unknown ones are counted so the
            // session row can say what current CLI versions add.
            if ![
                "queued_command",
                "compact_file_reference",
                "total_tokens_reminder",
            ]
            .contains(&kind)
            {
                parser.skipped.note("Claude attachment 类型", kind);
                return Ok(());
            }
            if tag.is_none()
                && attachment["type"] == "queued_command"
                && attachment["commandMode"] == "prompt"
                && attachment["origin"]["kind"] == "human"
                && attachment["prompt"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty())
            {
                parser.turn = first_nonempty(&[record["uuid"].clone()], &end.to_string());
                parser.emit(end, "user", string(&attachment["prompt"]), &ts, json!({}));
            }
        }
        "queue-operation" if tag.is_none() => {
            let operation = record["operation"].as_str().unwrap_or("");
            if ["enqueue", "remove", "dequeue", "popAll"].contains(&operation)
                && (!["enqueue", "remove"].contains(&operation)
                    || record["content"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()))
            {
                parser.emit(
                    end,
                    "queue_operation",
                    record["content"].as_str().unwrap_or(""),
                    &ts,
                    json!({"operation": operation, "counted": false, "silent": true}),
                );
                clear_turn(parser);
            }
        }
        "custom-title" if tag.is_none() => {
            let title = string(&record["customTitle"]);
            if !title.is_empty() && title != parser.previous_title {
                parser.emit(
                    end,
                    "command",
                    format!("/rename {title}"),
                    &ts,
                    json!({
                        "counted": false, "inferred": true,
                        "event_id": format!("rename:{}:{end}", string(&record["sessionId"])),
                    }),
                );
                clear_turn(parser);
                parser.previous_title = title;
            }
        }
        "last-prompt"
        | "ai-title"
        | "file-history-snapshot"
        | "progress"
        | "summary"
        | "queue-operation"
        | "custom-title" => {}
        // Current CLI versions add mode/permission-mode/atis-latch/
        // bridge-session/file-history-delta/agent-name/cost-state and more;
        // the reference adapter's if/elif chain ignores them all.
        other => parser.skipped.note("Claude 记录类型", other),
    }
    Ok(())
}

fn tag_value(text: &str, name: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let opening = format!("<{name}>");
    let closing = format!("</{name}>");
    let start = lower.find(&opening)? + opening.len();
    let end = lower[start..].find(&closing)? + start;
    Some(html_escape::decode_html_entities(text[start..end].trim()).into_owned())
}

fn local_command(text: &str) -> Option<String> {
    let name = tag_value(text, "command-name")?;
    if !name.starts_with('/') {
        return None;
    }
    let args = tag_value(text, "command-args").unwrap_or_default();
    Some(format!("{name} {args}").trim_end().to_owned())
}

fn bash_input(text: &str) -> Option<String> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    if !lower.starts_with("<bash-input>") || !lower.ends_with("</bash-input>") {
        return None;
    }
    let command = html_escape::decode_html_entities(&text[12..text.len() - 13]);
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    Some(if command.starts_with('!') {
        command.to_owned()
    } else {
        format!("! {command}")
    })
}

fn bash_output(text: &str) -> Option<(String, bool)> {
    let mut rest = text.trim();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut matched = false;
    while !rest.is_empty() {
        let lower = rest.to_ascii_lowercase();
        let stream = if lower.starts_with("<bash-stdout>") {
            "stdout"
        } else if lower.starts_with("<bash-stderr>") {
            "stderr"
        } else {
            return None;
        };
        let closing = format!("</bash-{stream}>");
        let end = lower.find(&closing)?;
        let value = html_escape::decode_html_entities(&rest[13..end])
            .trim_matches('\n')
            .to_owned();
        if !value.is_empty() {
            if stream == "stdout" {
                stdout.push(value)
            } else {
                stderr.push(value)
            }
        }
        rest = rest[end + closing.len()..].trim_start();
        matched = true;
    }
    if !matched {
        return None;
    }
    let stdout = stdout.join("\n");
    let stderr = stderr.join("\n");
    let has_stderr = !stderr.is_empty();
    let text = match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => format!("{stdout}\n\nstderr:\n{stderr}"),
        (false, true) => stdout,
        _ => stderr,
    };
    Some((text, has_stderr))
}

fn notification(text: &str) -> Option<(String, String, Option<String>)> {
    let trimmed = text.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    let tail = lower.strip_prefix("<task-notification>")?;
    if !tail.is_empty() && !tail.starts_with(char::is_whitespace) {
        return None;
    }
    let status = tag_value(text, "status").unwrap_or_default().to_lowercase();
    let summary = tag_value(text, "summary").unwrap_or_default();
    let details = tag_value(text, "result").filter(|value| !value.is_empty());
    Some((notification_summary(&summary, &status), status, details))
}

fn notification_summary(summary: &str, status: &str) -> String {
    let lower = summary.to_ascii_lowercase();
    for (prefix, suffix, label) in [
        ("monitor event: \"", "\"", "监控事件"),
        ("monitor \"", "\" stream ended", "监控结束"),
        ("agent \"", "\" finished", "子代理完成"),
        ("agent \"", "\" was stopped by user", "子代理已停止"),
    ] {
        if lower.starts_with(prefix)
            && lower.ends_with(suffix)
            && summary.len() >= prefix.len() + suffix.len()
        {
            return format!(
                "{label} · {}",
                &summary[prefix.len()..summary.len() - suffix.len()]
            );
        }
    }
    if lower.starts_with("agent \"")
        && let Some(end) = lower.find("\" failed").filter(|end| *end >= 7)
    {
        let name = &summary[7..end];
        let reason = summary[end + 8..].trim_start_matches(':').trim();
        return if reason.is_empty() {
            format!("子代理失败 · {name}")
        } else {
            format!("子代理失败 · {name} · {reason}")
        };
    }
    if !summary.is_empty() {
        return summary.to_owned();
    }
    match status {
        "completed" => "后台任务完成",
        "failed" => "后台任务失败",
        "killed" => "后台任务已停止",
        _ => "后台任务通知",
    }
    .to_owned()
}
