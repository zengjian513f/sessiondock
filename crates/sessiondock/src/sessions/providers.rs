//! Pure native-record projection. File discovery, inheritance cutoffs and
//! persistent timeline overrides are owned by the session/history layer.

mod claude;
pub(super) mod envelopes;
mod grok;
mod image_content;
#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod tests;
mod tools;

use super::records::native_images::{MediaContext, Sidecar};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::LazyLock;

use serde_json::{Value, json};

use super::Event;

pub(super) fn validate_grok_summary(summary: &Value) -> Result<(), String> {
    grok::validate(summary)
}

pub(super) fn grok_untitled_title() -> &'static str {
    grok::UNTITLED_TITLE
}

#[cfg(test)]
pub(super) fn parse(
    source: &str,
    path: &Path,
    records: &[(Value, u64)],
    summary: Option<&Value>,
    fallback: &str,
) -> (Value, Vec<Event>, Option<String>) {
    parse_with_options(
        source,
        path,
        records,
        summary,
        fallback,
        ParseOptions::default(),
    )
}

/// Everything the reference adapters skip silently, counted here and surfaced
/// as non-fatal `migration_warnings`: unknown native kinds (their `if/elif`
/// chains fall through), Codex `session_meta` after the first, complete
/// lines that are not JSON objects (`_iter_records`), and a Claude ancestor
/// walk that stops short of a root (`_active_lineage`). Hard failures
/// (scalar `content`, non-string text, invalid media, record/LF/file budgets)
/// still make the whole session `supported:false`.
#[derive(Default)]
pub(super) struct Skipped {
    /// Distinct labels in first-seen file order with their counts.
    kinds: Vec<(String, usize)>,
    /// Records/blocks of kinds beyond the listed bound.
    overflow: usize,
    /// Codex `session_meta` records after the first (copied ancestor metas
    /// of old-style forks and subagent rollouts; `and not meta`).
    duplicate_codex_meta: usize,
    /// Complete lines the record scanner could not decode as JSON objects.
    invalid_lines: usize,
    /// Free-form notes (Claude lineage truncation), in the order they arose.
    notes: Vec<String>,
}

/// Distinct unknown kinds listed per session; the rest are one summary line.
pub(super) const MAX_SKIPPED_KINDS: usize = 32;
const MAX_KIND_CHARS: usize = 64;

impl Skipped {
    pub(super) fn duplicate_codex_meta(&mut self) {
        self.duplicate_codex_meta += 1;
    }

    /// The row line the index summary produces for the same count; it leads
    /// the warnings so head/tail summary and full view agree on the order.
    pub(super) fn duplicate_codex_meta_warning(count: usize) -> String {
        format!("跳过重复的Codex session_meta ×{count}")
    }

    /// The scanner's count for the bytes this parse covers (whole file for a
    /// view; the row summary derives its own from head/tail).
    pub(super) fn invalid_lines(&mut self, count: usize) {
        self.invalid_lines = count;
    }

    pub(super) fn warn(&mut self, note: String) {
        self.notes.push(note);
    }

    /// `label` is `<provider> <category>：<kind>`; the kind is clipped so a
    /// hostile record cannot inflate the session row.
    pub(super) fn note(&mut self, category: &str, kind: &str) {
        let kind = if kind.is_empty() {
            "(空)".to_owned()
        } else if kind.chars().count() > MAX_KIND_CHARS {
            kind.chars().take(MAX_KIND_CHARS).collect::<String>() + "…"
        } else {
            kind.to_owned()
        };
        let label = format!("{category}：{kind}");
        if let Some((_, count)) = self.kinds.iter_mut().find(|(known, _)| *known == label) {
            *count += 1;
        } else if self.kinds.len() < MAX_SKIPPED_KINDS {
            self.kinds.push((label, 1));
        } else {
            self.overflow += 1;
        }
    }

    pub(super) fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.duplicate_codex_meta > 0 {
            out.push(Self::duplicate_codex_meta_warning(
                self.duplicate_codex_meta,
            ));
        }
        out.extend(
            self.kinds
                .iter()
                .map(|(label, count)| format!("跳过未知的{label} ×{count}")),
        );
        if self.overflow > 0 {
            out.push(format!(
                "另有 {} 条其他未知类型已跳过（超过 {MAX_SKIPPED_KINDS} 种，未逐一列出）",
                self.overflow
            ));
        }
        // Same position as in the row summary (`skipped_warnings`), so the
        // detail merge replaces the head/tail count instead of adding a line.
        out.extend(super::records::invalid_lines_warning(self.invalid_lines));
        out.extend(self.notes.iter().cloned());
        out
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct ParseOptions<'a> {
    pub agent: &'a str,
    pub declared_tip: Option<&'a str>,
    pub abandoned_after: u64,
    /// Complete lines the record scanner skipped before these records
    /// (reported, never re-derived: the projection only sees decoded rows).
    pub invalid_lines: usize,
}

#[cfg(test)]
pub(super) fn parse_agent(
    source: &str,
    path: &Path,
    records: &[(Value, u64)],
    summary: Option<&Value>,
    fallback: &str,
    agent: &str,
) -> (Value, Vec<Event>, Option<String>) {
    parse_with_options(
        source,
        path,
        records,
        summary,
        fallback,
        ParseOptions {
            agent,
            ..Default::default()
        },
    )
}

#[cfg(test)]
pub(super) fn parse_with_options(
    source: &str,
    path: &Path,
    records: &[(Value, u64)],
    summary: Option<&Value>,
    fallback: &str,
    options: ParseOptions<'_>,
) -> (Value, Vec<Event>, Option<String>) {
    parse_context(source, path, records, summary, fallback, options, None)
}

pub(super) fn parse_with_media(
    source: &str,
    path: &Path,
    records: &[(Value, u64)],
    summary: Option<&Value>,
    fallback: &str,
    sidecars: &BTreeMap<u64, Vec<Sidecar>>,
) -> (Value, Vec<Event>, Option<String>) {
    parse_agent_with_media(source, path, records, summary, fallback, "", sidecars)
}

pub(super) fn parse_agent_with_media(
    source: &str,
    path: &Path,
    records: &[(Value, u64)],
    summary: Option<&Value>,
    fallback: &str,
    agent: &str,
    sidecars: &BTreeMap<u64, Vec<Sidecar>>,
) -> (Value, Vec<Event>, Option<String>) {
    parse_options_with_media(
        source,
        path,
        records,
        summary,
        fallback,
        ParseOptions {
            agent,
            ..Default::default()
        },
        sidecars,
    )
}

pub(super) use claude::{PinOutcome, PinTargetError};

/// Pure application of a persisted Claude display pin; see `claude::apply_pin`.
pub(super) fn claude_pin(records: &[(Value, u64)], tip: &str, stale_end: u64) -> PinOutcome {
    claude::apply_pin(records, tip, stale_end)
}

/// Resolve and validate a pin target against the natural active lineage.
pub(super) fn claude_pin_target(
    records: &[(Value, u64)],
    target: &str,
) -> Result<String, PinTargetError> {
    claude::resolve_pin_target(records, target)
}

/// Production parse with explicit timeline options; the session layer owns
/// the persisted pin and passes only the derived pure options here.
pub(super) fn parse_options_with_media(
    source: &str,
    path: &Path,
    records: &[(Value, u64)],
    summary: Option<&Value>,
    fallback: &str,
    options: ParseOptions<'_>,
    sidecars: &BTreeMap<u64, Vec<Sidecar>>,
) -> (Value, Vec<Event>, Option<String>) {
    let context = match MediaContext::new(records, sidecars) {
        Ok(context) => context,
        Err(error) => {
            return (
                metadata(source, path, records, summary, fallback),
                Vec::new(),
                Some(error),
            );
        }
    };
    parse_context(
        source,
        path,
        records,
        summary,
        fallback,
        options,
        Some(context),
    )
}

fn parse_context<'a>(
    source: &str,
    path: &Path,
    records: &'a [(Value, u64)],
    summary: Option<&Value>,
    fallback: &str,
    options: ParseOptions<'_>,
    media: Option<MediaContext<'a>>,
) -> (Value, Vec<Event>, Option<String>) {
    let mut meta = metadata(source, path, records, summary, fallback);
    let lineage = (source == "claude").then(|| claude::lineage(records, options));
    if let Some(lineage) = &lineage {
        meta["_timeline_tip"] = json!(lineage.tip);
    }
    if source == "claude" && !options.agent.is_empty() {
        meta["sid"] = json!(options.agent);
        meta["_is_subagent"] = json!(true);
        meta["_agent_title"] = json!(
            summary
                .and_then(|value| value["description"].as_str())
                .filter(|title| !title.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!(
                    "子代理 {}",
                    options.agent.chars().take(8).collect::<String>()
                ))
        );
        meta["_agent_type"] = json!(
            summary
                .and_then(|value| value["agentType"].as_str())
                .filter(|kind| !kind.is_empty())
                .unwrap_or("subagent")
        );
        meta["title"] = meta["_agent_title"].clone();
    }
    let mut parser = Parser {
        source: source.to_owned(),
        media,
        ..Default::default()
    };
    parser.skipped.invalid_lines(options.invalid_lines);
    for note in lineage.iter().flat_map(|lineage| &lineage.warnings) {
        parser.skipped.warn(note.clone());
    }
    for (record, end) in records {
        let branch = if let Some(lineage) = &lineage {
            let id = claude::graph_id(record, options.agent);
            let abandoned = id.is_some_and(|id| lineage.abandoned.contains(id));
            // A graph node renders when it is on the
            // active chain or below an abandoned input (`abandoned | offshoot`).
            if let Some(id) = id
                && let Some(active) = &lineage.active
                && !active.contains(id)
                && !abandoned
                && !lineage.offshoot.contains(id)
            {
                continue;
            }
            claude::Branch {
                abandoned,
                deferred_abort: id.is_some_and(|id| lineage.deferred_abort.contains(id)),
            }
        } else {
            claude::Branch::default()
        };
        let result = match source {
            "claude" => claude::record(&mut parser, record, *end, options.agent, branch),
            "codex" => parser.codex(record, *end),
            "grok" => parser.grok(record, *end),
            _ => Err("未知原生数据源".to_owned()),
        };
        if let Err(reason) = result {
            return (meta, Vec::new(), Some(reason));
        }
    }
    // Non-fatal: the session stays supported; the row reports what was skipped.
    meta["migration_warnings"] = json!(parser.skipped.warnings());
    (meta, parser.events, None)
}

fn string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn normalized(value: &Value) -> Value {
    let date = if let Some(number) = value.as_f64() {
        let seconds = if number > 1e11 {
            number / 1000.0
        } else {
            number
        };
        chrono::DateTime::from_timestamp_millis((seconds * 1000.0) as i64)
    } else if let Some(text) = value.as_str() {
        chrono::DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|dt| dt.to_utc())
    } else {
        None
    };
    date.map(|dt| json!(dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)))
        .unwrap_or(Value::Null)
}

fn first_nonempty(values: &[Value], fallback: &str) -> String {
    values
        .iter()
        .filter_map(Value::as_str)
        .find(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn clip(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > 90 {
        normalized.chars().take(90).collect::<String>() + "…"
    } else {
        normalized
    }
}

/// Grok wraps a user turn as `<image_files>…</image_files>` blocks followed by
/// `<user_query>…</user_query>`; images arrive as structured parts. A message
/// sent while Grok was working or after interrupting the previous turn also
/// carries a protocol prefix (`The user sent a message while you were
/// working:` / `The user interrupted the previous turn:`) and suffix (`Make
/// sure to complete any unfinished tasks from previous turns.`) outside the
/// tags; none of it is the user's text. Match the reference adapter:
/// whole-text, case-insensitive, and only one leading and one trailing
/// newline of the body are removed, not the user's indentation; any other
/// text outside the envelope keeps the record verbatim.
static GROK_USER_QUERY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?is)^\s*(?:<image_files>.*?</image_files>\s*)*",
        r"(?:The user (?:sent a message while you were working|interrupted the previous turn):\s*)?",
        r"(?:<image_files>.*?</image_files>\s*)*",
        r"<user_query>(.*?)</user_query>\s*",
        r"(?:Make sure to complete any unfinished tasks from previous turns\.)?\s*",
        r"(?:<skill_information>.*?</skill_information>\s*)?$",
    ))
    .unwrap()
});

fn strip_grok_user_query(text: &str) -> String {
    let Some(captures) = GROK_USER_QUERY.captures(text) else {
        return text.to_owned();
    };
    let body = captures.get(1).map_or("", |m| m.as_str());
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .unwrap_or(body);
    body.strip_suffix("\r\n")
        .or_else(|| body.strip_suffix('\n'))
        .unwrap_or(body)
        .to_owned()
}

/// Claude's paste envelope repeats its id on the closing tag. Only unwrap a
/// complete matching pair; unknown/malformed markup and the pasted body stay
/// literal. This is display normalization, never a change to native input.
pub(super) fn claude_pasted_text(text: &str) -> String {
    static OPEN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<pasted_content id="([^"]+)">"#).unwrap());
    let mut out = String::with_capacity(text.len());
    let mut position = 0;
    while let Some(captures) = OPEN.captures_at(text, position) {
        let opening = captures.get(0).unwrap();
        let closing = format!("</pasted_content id=\"{}\">", &captures[1]);
        let Some(offset) = text[opening.end()..].find(&closing) else {
            out.push_str(&text[position..opening.end()]);
            position = opening.end();
            continue;
        };
        out.push_str(&text[position..opening.start()]);
        let body = &text[opening.end()..opening.end() + offset];
        let body = body
            .strip_prefix("\r\n")
            .or_else(|| body.strip_prefix('\n'))
            .unwrap_or(body);
        out.push_str(
            body.strip_suffix("\r\n")
                .or_else(|| body.strip_suffix('\n'))
                .unwrap_or(body),
        );
        position = opening.end() + offset + closing.len();
    }
    out.push_str(&text[position..]);
    out
}

fn timeline_protocol(text: &str) -> bool {
    let text = text.trim_start().to_lowercase();
    if let Some(heading) = text.strip_prefix('#')
        && (heading.trim_start().starts_with("agents.md instructions")
            || heading.trim_start().starts_with("global user guidance"))
    {
        return true;
    }
    [
        "<instructions>",
        "<user_info>",
        "<environment_context>",
        "<system-reminder>",
        "<command-name>",
        "<local-command-caveat>",
        "<local-command-stdout>",
        "<task-notification>",
        "<project_instructions>",
        "<user_instructions>",
        "caveat: the messages below were generated by the user while running local commands",
        "this session is being continued from a previous conversation",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}

fn codex_internal(payload: &Value, text: &str) -> bool {
    payload["role"] == "developer"
        || (payload["role"] == "user"
            && (timeline_protocol(text)
                || envelopes::recommended_plugins(text)
                || payload["internal_chat_message_metadata_passthrough"]["content_item_kinds"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "goal.internal_context"))))
}

fn strip_codex_abort_prefix(text: &str) -> String {
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if !lower.starts_with("<turn_aborted>") {
            break;
        }
        let Some(end) = lower.find("</turn_aborted>") else {
            break;
        };
        rest = trimmed[end + "</turn_aborted>".len()..].trim_start();
    }
    rest.to_owned()
}

fn visible_text(content: &Value) -> String {
    // Inspect only native text blocks. Image source/file data is not title text;
    // ordinary user-authored JSON inside a text block remains ordinary text.
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.as_str() {
                    return Some(text);
                }
                match item["type"].as_str().unwrap_or("") {
                    "text" | "input_text" | "output_text" | "summary_text" => item["text"].as_str(),
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(_)
            if matches!(
                content["type"].as_str(),
                Some("text" | "input_text" | "output_text" | "summary_text")
            ) =>
        {
            content["text"].as_str().unwrap_or("").to_owned()
        }
        _ => String::new(),
    }
}

fn metadata(
    source: &str,
    path: &Path,
    records: &[(Value, u64)],
    summary: Option<&Value>,
    fallback: &str,
) -> Value {
    if source == "grok" {
        return grok::metadata(path, summary.unwrap_or(&Value::Null), fallback);
    }
    let filename = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut sid = filename.clone();
    let mut title = String::new();
    let mut cwd = String::new();
    let mut created = Value::Null;
    let mut updated = Value::Null;
    let mut model = Value::Null;
    let mut branch = Value::Null;
    let mut custom_title = String::new();
    let mut ai_title = String::new();
    let mut fork = Value::Null;
    let mut codex_meta = None;
    let mut claude_sid_seen = false;
    for (record, _) in records {
        let ts = normalized(&record["timestamp"]);
        if !ts.is_null() {
            if created.is_null() {
                created = ts.clone()
            }
            updated = ts;
        }
        match source {
            "claude" => {
                if !claude_sid_seen
                    && let Some(value) = record["sessionId"].as_str().filter(|sid| !sid.is_empty())
                {
                    sid = value.to_owned();
                    claude_sid_seen = true;
                }
                if cwd.is_empty() {
                    cwd = string(&record["cwd"])
                }
                if branch.is_null() {
                    branch = record["gitBranch"].clone()
                }
                if record["type"] == "custom-title" {
                    custom_title = string(&record["customTitle"])
                }
                if record["type"] == "ai-title" {
                    ai_title = string(&record["aiTitle"])
                }
                if record["type"] == "user"
                    && title.is_empty()
                    && record["isMeta"] != true
                    && record["isCompactSummary"] != true
                    && !truthy(&record["isSidechain"])
                {
                    let text = visible_text(&record["message"]["content"]);
                    if !timeline_protocol(&text) {
                        title = text
                    }
                }
            }
            "codex" => {
                let payload = &record["payload"];
                if record["type"] == "session_meta" && codex_meta.is_none() {
                    codex_meta = Some(payload);
                    let agent = payload["thread_source"] == "subagent"
                        || payload["source"].get("subagent").is_some();
                    sid = first_nonempty(
                        &[
                            if agent {
                                payload["id"].clone()
                            } else {
                                payload["session_id"].clone()
                            },
                            payload["id"].clone(),
                        ],
                        &sid,
                    );
                    cwd = string(&payload["cwd"]);
                    let native_created = normalized(&payload["timestamp"]);
                    if !native_created.is_null() {
                        created = native_created
                    }
                    fork = payload["forked_from_id"].clone();
                }
                if record["type"] == "turn_context" && model.is_null() {
                    model = payload["model"].clone()
                }
                if record["type"] == "response_item"
                    && payload["role"] == "user"
                    && title.is_empty()
                {
                    let text = strip_codex_abort_prefix(&visible_text(
                        &envelopes::codex_title_content(&payload["content"]),
                    ));
                    if !codex_internal(payload, &text) {
                        title = text
                    }
                }
            }
            _ => {}
        }
    }
    if !custom_title.is_empty() {
        title = custom_title
    } else if !ai_title.is_empty() {
        title = ai_title
    }
    if title.trim().is_empty() {
        title = filename
    }
    if cwd.is_empty() {
        cwd = "(未知)".to_owned()
    }
    if created.is_null() {
        created = json!(fallback)
    }
    if updated.is_null() {
        updated = created.clone()
    }
    let mut meta = json!({
        "sid": sid, "title": clip(&title), "cwd": cwd, "created": created,
        "updated": updated, "model": model, "branch": branch,
    });
    if !fork.is_null() {
        meta["forked_from_id"] = fork
    }
    if let Some(native) = codex_meta {
        let spawn = &native["source"]["subagent"]["thread_spawn"];
        let agent =
            native["thread_source"] == "subagent" || native["source"].get("subagent").is_some();
        meta["history_base"] = if native["history_base"].is_object() {
            native["history_base"].clone()
        } else {
            Value::Null
        };
        meta["forked_from_id"] = json!(string(&native["forked_from_id"]));
        meta["_is_subagent"] = json!(agent);
        meta["_parent_thread_id"] = json!(if agent {
            first_nonempty(
                &[
                    native["parent_thread_id"].clone(),
                    spawn["parent_thread_id"].clone(),
                    native["forked_from_id"].clone(),
                ],
                "",
            )
        } else {
            String::new()
        });
        meta["_agent_title"] = json!(first_nonempty(
            &[
                native["agent_path"].clone(),
                spawn["agent_path"].clone(),
                native["agent_nickname"].clone(),
                spawn["agent_nickname"].clone(),
            ],
            meta["sid"].as_str().unwrap_or("")
        ));
        meta["_agent_type"] = json!(first_nonempty(
            &[native["agent_role"].clone(), spawn["agent_role"].clone()],
            "subagent"
        ));
    }
    meta
}

#[derive(Default)]
struct Parser<'a> {
    events: Vec<Event>,
    calls: HashMap<String, String>,
    turn: String,
    previous_title: String,
    codex_meta_seen: bool,
    source: String,
    media: Option<MediaContext<'a>>,
    skipped: Skipped,
}

impl Parser<'_> {
    fn emit(&mut self, end: u64, role: &str, text: impl Into<String>, ts: &Value, extra: Value) {
        let mut message = json!({"role": role, "text": text.into(), "ts": ts,
                                 "name": null, "args": null});
        if !self.turn.is_empty() {
            message["turn_id"] = json!(self.turn)
        }
        if let Some(fields) = extra.as_object() {
            for (key, value) in fields {
                message[key] = value.clone()
            }
        }
        self.events.push(Event {
            end,
            message,
            media: Vec::new(),
        });
    }

    fn emit_media(
        &mut self,
        end: u64,
        role: &str,
        text: String,
        ts: &Value,
        extra: Value,
        media: Vec<crate::media::NativeImage>,
    ) -> Result<(), String> {
        self.emit(end, role, text, ts, extra);
        self.events.last_mut().expect("emit adds an event").media = media;
        Ok(())
    }

    fn status(&mut self, end: u64, state: &str, ts: &Value, extra: Value) {
        let mut fields = if extra.is_object() { extra } else { json!({}) };
        fields["state"] = json!(state);
        self.emit(end, "status", state, ts, fields);
    }

    fn codex(&mut self, record: &Value, end: u64) -> Result<(), String> {
        let kind = record["type"].as_str().unwrap_or("");
        let payload = &record["payload"];
        let ts = normalized(&record["timestamp"]);
        if kind == "session_meta" {
            // Only the first
            // session_meta is this file's identity; old-style forks and
            // subagent rollouts copy their ancestors' metas after it.
            if self.codex_meta_seen {
                self.skipped.duplicate_codex_meta();
                return Ok(());
            }
            if !payload["history_base"].is_null() && !payload["history_base"].is_object() {
                return Err(
                    "Codex history_base 必须是对象或 null；拒绝忽略损坏的继承身份".to_owned(),
                );
            }
            self.codex_meta_seen = true;
            return Ok(());
        }
        if kind == "turn_context" {
            return Ok(());
        }
        if kind == "compacted" {
            self.turn.clear();
            let id = if truthy(&record["ordinal"]) {
                string(&record["ordinal"])
            } else {
                first_nonempty(std::slice::from_ref(&ts), &end.to_string())
            };
            self.emit(end, "event", "已压缩", &ts,
                      json!({"counted": false, "event_kind": "compact", "event_id": format!("compact:{id}")}));
            return Ok(());
        }
        if kind != "event_msg" && kind != "response_item" {
            // e.g. token_usage_record / inter_agent_communication_metadata:
            // the reference adapter falls through without a message.
            self.skipped.note("Codex 记录类型", kind);
            return Ok(());
        }
        let native_turn = first_nonempty(
            &[
                payload["turn_id"].clone(),
                payload["internal_chat_message_metadata_passthrough"]["turn_id"].clone(),
            ],
            "",
        );
        // Codex turn identity belongs to each native record. Missing identity
        // is not permission to attach a new input to an earlier task_started.
        self.turn = native_turn;
        if kind == "event_msg" {
            match payload["type"].as_str().unwrap_or("") {
                "task_started" => self.status(end, "working", &ts, json!({})),
                "task_complete" => self.status(
                    end,
                    if truthy(&payload["error"]) {
                        "failed"
                    } else {
                        "idle"
                    },
                    &ts,
                    json!({"duration_ms": payload["duration_ms"]}),
                ),
                "turn_aborted" => {
                    for event in self.events.iter_mut().rev() {
                        if event.message["role"] == "assistant"
                            && event.message["turn_id"] == self.turn
                        {
                            if event.message["phase"] != "final" {
                                event.message["interrupted"] = json!(true);
                                event.message["interrupt_reason"] = json!(first_nonempty(
                                    &[payload["reason"].clone()],
                                    "本轮在最终答复前被中断"
                                ));
                            }
                            break;
                        }
                    }
                    self.status(
                        end,
                        "aborted",
                        &ts,
                        json!({"reason": payload["reason"], "duration_ms": payload["duration_ms"]}),
                    );
                }
                // These notifications duplicate response_item or are telemetry.
                "user_message" | "agent_message" | "agent_reasoning" | "token_count" => {}
                // e.g. item_completed: skipped like the reference adapter.
                other => self.skipped.note("Codex event_msg", other),
            }
            return Ok(());
        }
        match payload["type"].as_str().unwrap_or("") {
            "message" => {
                let native_role = payload["role"].as_str().unwrap_or("user");
                let role = match native_role {
                    "developer" => "system",
                    "tool" => "tool_result",
                    other => other,
                };
                let text = visible_text(&payload["content"]);
                let text = if native_role == "user" {
                    strip_codex_abort_prefix(&text)
                } else {
                    text
                };
                if codex_internal(payload, &text) {
                    return Ok(());
                }
                let phase = match payload["phase"].as_str() {
                    Some("final_answer") => Some("final"),
                    Some("commentary") => Some("progress"),
                    _ => None,
                };
                let mut extra = json!({});
                if let Some(phase) = phase {
                    extra["phase"] = json!(phase)
                }
                let parts_with_media = if native_role == "user" {
                    image_content::codex_parts_with_media
                } else {
                    image_content::parts_with_media
                };
                let (parts, media) =
                    parts_with_media(&payload["content"], self.media.as_ref(), &mut self.skipped)?;
                let parts = if native_role == "user" {
                    strip_codex_abort_prefix(&parts)
                } else {
                    parts
                };
                if !parts.trim().is_empty() {
                    self.emit_media(end, role, parts, &ts, extra, media)?;
                }
            }
            "reasoning" => {
                let text = text_parts(&payload["summary"], &mut self.skipped)?;
                if !text.trim().is_empty() {
                    self.emit(end, "thinking", text, &ts, json!({}))
                }
            }
            "function_call" | "custom_tool_call" | "local_shell_call" => {
                let name = first_nonempty(
                    &[payload["name"].clone()],
                    payload["type"].as_str().unwrap_or("tool"),
                );
                let input = ["arguments", "input", "action"]
                    .into_iter()
                    .map(|key| &payload[key])
                    .find(|v| !v.is_null())
                    .unwrap_or(&Value::Null);
                self.tool(end, &name, &string(&payload["call_id"]), input, &ts)?;
            }
            "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
                self.output(
                    end,
                    &string(&payload["call_id"]),
                    &payload["output"],
                    false,
                    &ts,
                )?;
            }
            "web_search_call" | "tool_search_call" => {
                // The reference adapter pretty-prints `arguments or {}`, so a
                // missing/empty argument renders as "{}" and a JSON string is
                // re-indented; other strings are shown as written.
                let arguments = match &payload["arguments"] {
                    Value::Null => "{}".to_owned(),
                    Value::String(text) if text.is_empty() => "{}".to_owned(),
                    Value::String(text) => serde_json::from_str::<Value>(text)
                        .ok()
                        .and_then(|value| serde_json::to_string_pretty(&value).ok())
                        .unwrap_or_else(|| text.clone()),
                    other => string(other),
                };
                self.emit(
                    end,
                    "tool",
                    arguments,
                    &ts,
                    json!({"name": payload["type"]}),
                );
            }
            // e.g. agent_message: skipped like the reference adapter.
            other => self.skipped.note("Codex response_item", other),
        }
        Ok(())
    }

    fn grok(&mut self, record: &Value, end: u64) -> Result<(), String> {
        let kind = record["type"].as_str().unwrap_or("");
        let ts = normalized(&record["timestamp"]);
        match kind {
            "reasoning" => {
                let text = text_parts(&record["summary"], &mut self.skipped)?;
                if !text.trim().is_empty() {
                    self.emit(end, "thinking", text, &ts, json!({}))
                }
            }
            "tool_result" => self.output(
                end,
                &string(&record["tool_call_id"]),
                &record["content"],
                false,
                &ts,
            )?,
            "user" | "assistant" | "system" => {
                let (mut text, media) = image_content::parts_with_media(
                    &record["content"],
                    self.media.as_ref(),
                    &mut self.skipped,
                )?;
                let mut user_query = false;
                if kind == "user" {
                    let unwrapped = strip_grok_user_query(&text);
                    user_query = unwrapped != text;
                    text = unwrapped;
                }
                // Grok CLI writes the session preamble as the first
                // `type: system` record (no synthetic_reason). Real
                // histories only use this kind for that preamble; it is
                // not conversation. Python still emits it (DELTA).
                if kind != "system"
                    && !truthy(&record["synthetic_reason"])
                    && !text.trim().is_empty()
                    && !(kind == "user" && !user_query && timeline_protocol(&text))
                {
                    if kind == "user" {
                        self.turn = if record["prompt_index"].is_null() {
                            format!("offset:{end}")
                        } else {
                            format!("prompt:{}", record["prompt_index"])
                        };
                    }
                    let extra = if kind == "assistant" {
                        json!({"phase": if record["tool_calls"].as_array().is_some_and(|calls| !calls.is_empty()) { "progress" } else { "final" }})
                    } else {
                        json!({})
                    };
                    self.emit_media(end, kind, text, &ts, extra, media)?;
                }
                if let Some(calls) = record["tool_calls"].as_array() {
                    for call in calls {
                        let name = first_nonempty(
                            &[call["name"].clone(), call["function"]["name"].clone()],
                            "tool",
                        );
                        let input = if call["arguments"].is_null() {
                            &call["function"]["arguments"]
                        } else {
                            &call["arguments"]
                        };
                        self.tool(end, &name, &string(&call["id"]), input, &ts)?;
                    }
                }
            }
            // Skipped like the reference adapter's if/elif chain.
            other => self.skipped.note("Grok 记录类型", other),
        }
        Ok(())
    }

    fn content(
        &mut self,
        end: u64,
        role: &str,
        content: &Value,
        ts: &Value,
        phase: Option<&str>,
    ) -> Result<(), String> {
        let items = match content {
            Value::Null => return Ok(()),
            Value::Array(items) => items.as_slice(),
            Value::String(_) | Value::Object(_) => std::slice::from_ref(content),
            _ => return Err("原生 content 类型无效".to_owned()),
        };
        for part in items {
            if let Some(image) = image_content::native_image(part, self.media.as_ref())? {
                let mut extra = json!({});
                if let Some(phase) = phase {
                    extra["phase"] = json!(phase);
                }
                self.emit_media(end, role, "[图片]".into(), ts, extra, vec![image])?;
                continue;
            }
            if let Some(text) = part.as_str() {
                if text.trim().is_empty() || (role == "user" && timeline_protocol(text)) {
                    continue;
                }
                let mut extra = json!({});
                if let Some(phase) = phase {
                    extra["phase"] = json!(phase)
                }
                let (text, media) =
                    image_content::parts_with_media(part, self.media.as_ref(), &mut self.skipped)?;
                self.emit_media(end, role, text, ts, extra, media)?;
                continue;
            }
            match part["type"].as_str().unwrap_or("") {
                "text" | "input_text" | "output_text" | "summary_text" => {
                    let (text, media) = image_content::parts_with_media(
                        part,
                        self.media.as_ref(),
                        &mut self.skipped,
                    )?;
                    if role == "user" && timeline_protocol(&text) {
                        continue;
                    }
                    if !text.trim().is_empty() {
                        let mut extra = json!({});
                        if let Some(phase) = phase {
                            extra["phase"] = json!(phase)
                        }
                        self.emit_media(end, role, text, ts, extra, media)?;
                    }
                }
                "thinking" => {
                    let text = string(&part["thinking"]);
                    if !text.trim().is_empty() {
                        self.emit(end, "thinking", text, ts, json!({}))
                    }
                }
                "tool_use" => self.tool(
                    end,
                    &first_nonempty(&[part["name"].clone()], "tool"),
                    &string(&part["id"]),
                    &part["input"],
                    ts,
                )?,
                "tool_result" => self.output(
                    end,
                    &string(&part["tool_use_id"]),
                    &part["content"],
                    part["is_error"] == true,
                    ts,
                )?,
                // Non-image blocks of an unknown kind are skipped like the
                // reference `_flatten_content`; image-shaped blocks that fail
                // to decode were already rejected above.
                other => self.skipped.note("内容块类型", other),
            }
        }
        Ok(())
    }

    fn tool(
        &mut self,
        end: u64,
        name: &str,
        call_id: &str,
        input: &Value,
        ts: &Value,
    ) -> Result<(), String> {
        self.calls.insert(call_id.to_owned(), name.to_owned());
        let decoded = input
            .as_str()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .unwrap_or_else(|| input.clone());
        if let Some((text, questions)) = tools::question(name, input) {
            let fields = if self.source == "grok" {
                json!({"questions": questions})
            } else {
                json!({"name": name, "call_id": call_id, "questions": questions})
            };
            self.emit(end, "question", text, ts, fields);
            self.status(end, "waiting", ts, json!({}));
        } else {
            let mut fields = json!({"name": name, "call_id": call_id, "summary": tools::summary(name, input), "changes": null});
            match tools::changes(name, input) {
                Ok(changes) if !changes.is_empty() => fields["changes"] = json!(changes),
                Ok(_) => {}
                Err(reason) => fields["changes_unavailable_reason"] = json!(reason),
            }
            self.emit(end, "tool", string(&decoded), ts, fields);
        }
        Ok(())
    }

    fn output(
        &mut self,
        end: u64,
        call_id: &str,
        output: &Value,
        error: bool,
        ts: &Value,
    ) -> Result<(), String> {
        let name = self.calls.get(call_id).cloned();
        let answer = self.source != "grok" && name.as_deref().is_some_and(is_question);
        let error = error
            || self
                .media
                .as_ref()
                .and_then(|context| context.tool_error(output))
                == Some(true)
            || (image_content::tool_wrapper_with_media(output, self.media.as_ref())
                && output["isError"] == true);
        // Remove recognized embedded blocks before any tool-envelope JSON is
        // rendered as text. Keep metadata parsing against the scrubbed shape.
        let (cleaned, media) = if self.source == "codex" {
            tools::sanitize_output_with_media(output, self.media.as_ref())?
        } else {
            image_content::sanitize_tool_with_media(output, self.media.as_ref())?
        };
        let output = &cleaned;
        let (mut text, mut fields) = if self.source == "codex" {
            tools::output(output)?
        } else {
            (
                tool_text_parts(
                    if image_content::tool_wrapper(output) {
                        &output["content"]
                    } else {
                        output
                    },
                    &mut self.skipped,
                )?,
                json!({}),
            )
        };
        let mut failed = error || fields["error"] == true;
        if let Some(code) = fields["exit_code"]
            .as_i64()
            .or_else(|| output_exit_code(&text))
        {
            failed |= code != 0;
            fields["exit_code"] = json!(code);
        }
        if answer {
            if self.source == "codex" {
                let (answer, cancelled) = tools::answer(output)?;
                text = answer;
                failed |= cancelled;
            } else if error
                && text.starts_with("The user doesn't want to proceed with this tool use.")
            {
                text = "已取消回答".to_owned();
                failed = true;
            }
        }
        text = envelopes::tool_text(&self.source, name.as_deref(), &text);
        fields["name"] = json!(name);
        fields["call_id"] = json!(call_id);
        fields["error"] = json!(failed);
        let text = image_content::placeholder(text, &media);
        self.emit_media(
            end,
            if answer { "answer" } else { "tool_result" },
            text,
            ts,
            fields,
            media,
        )?;
        if answer {
            self.status(end, "working", ts, json!({}))
        }
        Ok(())
    }
}

/// Exit code over the first 400 characters, case-insensitively:
/// `"exit_code"\s*:\s*(-?\d+)` or `\bexit(?:ed)?(?: with)?(?: code| status)? (-?\d+)`
/// — the plain form needs exactly one space before the number, so `exit: 1`
/// (Grok's output header) is not a code.
fn output_exit_code(text: &str) -> Option<i64> {
    fn integer(value: &str) -> Option<i64> {
        let end = value
            .char_indices()
            .take_while(|(index, ch)| ch.is_ascii_digit() || (*index == 0 && *ch == '-'))
            .map(|(index, ch)| index + ch.len_utf8())
            .last()?;
        value[..end].parse().ok()
    }
    let prefix = text.chars().take(400).collect::<String>().to_lowercase();
    if let Some((_, rest)) = prefix.split_once("\"exit_code\"")
        && let Some(rest) = rest.trim_start().strip_prefix(':')
        && let Some(code) = integer(rest.trim_start())
    {
        return Some(code);
    }
    for (index, _) in prefix.match_indices("exit") {
        if prefix[..index]
            .chars()
            .last()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        {
            continue;
        }
        let mut rest = &prefix[index + 4..];
        if let Some(tail) = rest.strip_prefix("ed") {
            rest = tail
        }
        if let Some(tail) = rest.strip_prefix(" with") {
            rest = tail
        }
        if let Some(tail) = rest
            .strip_prefix(" code")
            .or_else(|| rest.strip_prefix(" status"))
        {
            rest = tail
        }
        if let Some(tail) = rest.strip_prefix(' ')
            && let Some(code) = integer(tail)
        {
            return Some(code);
        }
    }
    None
}

fn is_question(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "askuserquestion"
        || lower == "request_user_input"
        || lower.ends_with(".request_user_input")
        || lower.ends_with("__request_user_input")
}

/// Claude/Grok tool results drop empty text blocks before joining, exactly
/// like the reference adapter; ordinary message content keeps them.
fn tool_text_parts(content: &Value, skipped: &mut Skipped) -> Result<String, String> {
    match content {
        Value::Array(items) => Ok(items
            .iter()
            .map(|item| text_part(item, skipped))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n")),
        other => text_parts(other, skipped),
    }
}

/// Text of a content value. Blocks of an unknown non-image kind (and
/// non-string/non-object array elements) are skipped and counted, like the
/// reference `_flatten_content`; a text block whose `text` is not a string and
/// a top-level scalar remain hard failures (the reference adapter raises).
fn text_parts(content: &Value, skipped: &mut Skipped) -> Result<String, String> {
    Ok(text_part(content, skipped)?.unwrap_or_default())
}

/// `Ok(None)` is a skipped block: it contributes no line to a joined array.
fn text_part(content: &Value, skipped: &mut Skipped) -> Result<Option<String>, String> {
    match content {
        Value::Null => Ok(Some(String::new())),
        Value::String(text) => Ok(Some(text.clone())),
        Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                if item.is_string() || item.is_object() || item.is_array() {
                    if let Some(part) = text_part(item, skipped)? {
                        parts.push(part);
                    }
                } else {
                    skipped.note("内容块类型", "(非对象元素)");
                }
            }
            Ok(Some(parts.join("\n")))
        }
        Value::Object(_) => match content["type"].as_str().unwrap_or("") {
            "text" | "input_text" | "output_text" | "summary_text" => match &content["text"] {
                Value::Null => Ok(Some(String::new())),
                Value::String(text) => Ok(Some(text.clone())),
                _ => Err("原生文本块的 text 必须是字符串".into()),
            },
            other => {
                skipped.note("内容块类型", other);
                Ok(None)
            }
        },
        _ => Err("原生 content 类型无效".to_owned()),
    }
}
