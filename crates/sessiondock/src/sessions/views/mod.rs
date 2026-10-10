//! Per-session views on demand: one opened session is
//! streamed through the existing checked, incremental record path and kept in
//! a bounded LRU; the session list never builds views. Design: docs/read-model.md.
//!
//! The unit of work is ONE candidate file: `parse_candidate` streams it through
//! `RecordCache`/`CheckedNative`/`RawIndex`/provider projection into an
//! immutable [`Parsed`]; [`Views`] composes a [`ViewSnapshot`] from that leaf,
//! its inherited fixed prefixes (Codex `history_base` parents, obtained through
//! the caller's [`Dependencies`] callback — never by joining paths here) and
//! the owner's published list row. Appends extend from the last committed
//! offset (full old-prefix digest verification, reused ASTs); rewrites,
//! truncation, stamp or pin changes rebuild; a file that changes while it is
//! being read keeps its existing per-session retry codes (503/409).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::Instant;

use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use super::{
    CURSOR_SCHEMA, Candidate, MessageQuery, NativeScope, PageStore, RewindTarget, SessionError,
    budgets, claude_agent_of, hash, history, media_projection, native_input, pages, providers,
    records, restamp, scope, timestamp, uid_for,
};
use crate::metadata::TimelinePin;

pub(crate) mod body;
mod encoded;
pub(crate) use body::MessageBody;
pub(crate) use encoded::EncodedEvents;

/// LRU bounds of the view cache (default 16 entries / 128 MiB; docs/read-model.md).
/// Runtime view budgets: `SESSIONDOCK_CACHE_ENTRIES` /
/// `SESSIONDOCK_VIEW_CACHE_MB`.
fn view_limit() -> usize {
    budgets::caches().view_entries
}
fn view_byte_limit() -> usize {
    budgets::caches().view_bytes
}
#[derive(Clone)]
pub(crate) struct Event {
    pub end: u64,
    pub message: Value,
    // Private payloads are never part of serialized messages/search/logs.
    pub media: Vec<crate::media::NativeImage>,
}

/// A non-status event chosen for projection together with its absolute index
/// among the view's non-status events, so a media grant can re-find it later.
#[derive(Clone, Copy)]
pub(crate) struct Selected<'a> {
    pub index: usize,
    pub event: &'a Event,
}

/// One candidate file, fully streamed and projected. Immutable once built;
/// the AST it came from lives (bounded) in the shared `RecordCache` only to
/// speed up the next incremental extension.
pub(crate) struct Parsed {
    pub candidate: Candidate,
    pub raw_index: native_input::RawIndex,
    pub committed: usize,
    pub meta: Value,
    /// Errors apply only to native-scope consumers, not compatible history display.
    pub native_id: Result<String, SessionError>,
    pub events: Vec<Event>,
    /// The serialized form of `events` (docs/read-model.md "视图字节缓存"):
    /// exact message bytes for hot reads, the committed semantic digest and
    /// the LRU accounting, all from one serialization pass at parse time.
    pub encoded: EncodedEvents,
    pub unsupported: Option<String>,
    pub raw_error: Option<String>,
    /// The persisted Claude display pin these events were projected with
    /// (main sessions only). A different pin means a different logical view.
    pub pin: Option<TimelinePin>,
    /// Small forward-reducer state, charged to the view LRU. Native ASTs stay
    /// exclusively in RecordCache; eviction there prevents projection reuse.
    append_projection: Option<providers::AppendProjection>,
}

impl Parsed {
    pub fn prefix_hash(&self, end: usize) -> String {
        self.raw_index
            .prefix_hash(end as u64)
            .expect("caller validated the physical JSONL checkpoint")
    }
    pub fn head(&self, end: usize) -> String {
        head(self.raw_index.head_bytes(), end)
    }
    /// `projection_digest(events, committed)`, computed while encoding.
    pub fn semantic_digest(&self) -> &str {
        self.encoded.digest()
    }
    /// Serialized message bytes plus resident media, the unit of the view
    /// budgets (the retained JSON bytes are the same figure, so a view is
    /// charged once for both its tree and its bytes; see docs/read-model.md).
    fn encoded_bytes(&self) -> usize {
        accounted_bytes(&self.encoded, self.events.iter()).saturating_add(
            self.append_projection
                .as_ref()
                .map_or(0, providers::AppendProjection::retained_weight),
        )
    }
}

/// Serialized message bytes plus resident media of one encoded event list.
fn accounted_bytes<'a>(encoded: &EncodedEvents, events: impl Iterator<Item = &'a Event>) -> usize {
    events
        .flat_map(|event| event.media.iter())
        .map(crate::media::NativeImage::resident_len)
        .fold(encoded.total_len(), usize::saturating_add)
}

/// Encode a projected event list, retaining its bytes for hot reads.
fn encode_events(
    events: &[Event],
    retain: bool,
    previous: Option<encoded::Previous<'_>>,
) -> Result<EncodedEvents, SessionError> {
    EncodedEvents::build(events, retain, previous)
        .map_err(|_| SessionError::new(500, "消息序列化失败"))
}

/// The resolved logical view behind a [`ViewSnapshot`]: one parsed leaf plus
/// the fixed inherited prefix events (physical end zero) that precede it.
pub(crate) struct View {
    pub parsed: Arc<Parsed>,
    pub meta: Value,
    pub inherited: Arc<Vec<Event>>,
    /// The serialized form of `inherited`, shared with the previous build
    /// of this view when the chain was reused.
    pub inherited_encoded: Arc<EncodedEvents>,
    pub identity: String,
    pub native_scope: Result<NativeScope, SessionError>,
    /// Every candidate file this view was projected from, leaf first, then
    /// inherited parents. Native span reads authorize against these stamps.
    pub sources: Vec<Candidate>,
    /// Owner UIDs whose entries this view depends on (leaf, owner, parents).
    pub dependencies: Vec<String>,
    /// A Codex main session renamed through the
    /// name index shows the local `/rename <name>` as an inferred, uncounted
    /// `command` event at `renamed_at` (before the first later message).
    /// Derived from `meta` alone, so a rename recomposes without a reparse;
    /// its physical end is 0 — a full read carries it, an append never does.
    pub rename: Option<Event>,
    /// The non-status position the rename event occupies in `events()`.
    pub rename_at: Option<usize>,
}

/// Where a non-status position of a view's `events()` lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Inherited(usize),
    Leaf(usize),
    Rename,
}

/// The `View` fields every constructor derives the same way.
pub(crate) struct ViewParts {
    pub parsed: Arc<Parsed>,
    pub meta: Value,
    pub inherited: Arc<Vec<Event>>,
    pub inherited_encoded: Arc<EncodedEvents>,
    pub identity: String,
    pub native_scope: Result<NativeScope, SessionError>,
    pub sources: Vec<Candidate>,
    pub dependencies: Vec<String>,
}

impl View {
    /// Compose a view; the rename event and its position follow from `meta`.
    pub(crate) fn new(parts: ViewParts) -> Self {
        let rename = rename_event(&parts.meta);
        let rename_at = rename.as_ref().map(|rename| {
            let at = rename.message["ts"].as_str().unwrap_or("");
            let mut position = 0;
            for event in parts.inherited.iter().chain(parts.parsed.events.iter()) {
                if event.message["ts"].as_str().is_some_and(|ts| ts > at) {
                    break;
                }
                if event.message["role"] != "status" {
                    position += 1;
                }
            }
            position
        });
        Self {
            parsed: parts.parsed,
            meta: parts.meta,
            inherited: parts.inherited,
            inherited_encoded: parts.inherited_encoded,
            identity: parts.identity,
            native_scope: parts.native_scope,
            sources: parts.sources,
            dependencies: parts.dependencies,
            rename,
            rename_at,
        }
    }

    /// Non-status position → the event list it lives in and its index there.
    pub(crate) fn slot(&self, position: usize) -> Slot {
        let position = match self.rename_at {
            Some(at) if position == at => return Slot::Rename,
            Some(at) if position > at => position - 1,
            _ => position,
        };
        let inherited = self.inherited_encoded.message_count();
        if position < inherited {
            Slot::Inherited(position)
        } else {
            Slot::Leaf(position - inherited)
        }
    }

    /// The encoder's entry for `event` at non-status `position`, proven by
    /// identity: the event list the position maps to must hold this very
    /// event there (`None` for the rename event, or should a caller's
    /// position ever disagree with the view's numbering — then the event is
    /// measured and serialized from itself, never from another's bytes).
    pub(crate) fn entry(&self, position: usize, event: &Event) -> Option<&encoded::Entry> {
        let (encoded, events, local): (&EncodedEvents, &[Event], usize) = match self.slot(position)
        {
            Slot::Inherited(index) => (&self.inherited_encoded, &self.inherited, index),
            Slot::Leaf(index) => (&self.parsed.encoded, &self.parsed.events, index),
            Slot::Rename => return None,
        };
        let held = events.get(encoded.event_index(local)?)?;
        std::ptr::eq(held, event).then(|| encoded.message_entry(local))?
    }
}

/// The `/rename` command event
/// of a renamed Codex main view (`event_id` `rename:<sid>:<renamed_at>`, the
/// id the frontend also uses to merge its own copy).
pub(crate) fn rename_event(meta: &Value) -> Option<Event> {
    if meta["source"] != "codex" || meta["agent_id"].as_str().is_some_and(|id| !id.is_empty()) {
        return None;
    }
    let at = meta["renamed_at"].as_str().filter(|at| !at.is_empty())?;
    let name = meta["renamed_to"]
        .as_str()
        .filter(|name| !name.is_empty())?;
    let sid = meta["sid"].as_str().unwrap_or("");
    Some(Event {
        end: 0,
        message: json!({
            "role": "command", "text": format!("/rename {name}"), "ts": at,
            "name": null, "args": null, "counted": false, "inferred": true,
            "event_id": format!("rename:{sid}:{at}"),
        }),
        media: Vec::new(),
    })
}

/// The view's events with the rename event (if any) spliced in before the
/// first event whose `ts` is later than the rename.
struct WithRename<'a, I: Iterator<Item = &'a Event>> {
    base: std::iter::Peekable<I>,
    rename: Option<&'a Event>,
}

impl<'a, I: Iterator<Item = &'a Event>> Iterator for WithRename<'a, I> {
    type Item = &'a Event;
    fn next(&mut self) -> Option<&'a Event> {
        if let Some(rename) = self.rename {
            let at = rename.message["ts"].as_str().unwrap_or("");
            let later = self
                .base
                .peek()
                .is_none_or(|next| next.message["ts"].as_str().is_some_and(|ts| ts > at));
            if later {
                self.rename = None;
                return Some(rename);
            }
        }
        self.base.next()
    }
}

impl View {
    pub fn events(&self) -> impl Iterator<Item = &Event> + '_ {
        WithRename {
            base: self
                .inherited
                .iter()
                .chain(self.parsed.events.iter())
                .peekable(),
            rename: self.rename.as_ref(),
        }
    }

    fn texts(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.events().filter_map(|event| {
            let role = event.message["role"].as_str()?;
            if role == "status" || event.message["inferred"] == true {
                return None;
            }
            Some((role, event.message["text"].as_str().unwrap_or("")))
        })
    }
}

/// Immutable logical view. It owns no open files, and producing a client batch
/// never re-enters the inventory or reparses native history. Heavy serialization
/// must still run on the bounded reader worker.
pub struct ViewSnapshot {
    pub(crate) view: Arc<View>,
    pub(crate) head: String,
    pub(crate) anchor: String,
    revision: String,
}

impl std::fmt::Debug for ViewSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the messages: identity and cursor only.
        f.debug_struct("ViewSnapshot")
            .field("uid", &self.view.meta["uid"])
            .field("agent_id", &self.view.meta["agent_id"])
            .field("end", &self.view.parsed.committed)
            .field("revision", &self.revision)
            .finish()
    }
}

impl ViewSnapshot {
    pub(crate) fn new(view: Arc<View>) -> Self {
        let head = view.parsed.head(view.parsed.committed);
        let anchor = anchor(&view, view.parsed.committed);
        let revision = hash(
            &serde_json::to_vec(&json!([
                view.meta,
                head,
                anchor,
                view.parsed.raw_index.length(),
                view.parsed
                    .candidate
                    .data_stamp()
                    .map(|stamp| stamp.modified.to_string()),
            ]))
            .expect("JSON snapshot identity"),
        );
        Self {
            view,
            head,
            anchor,
            revision,
        }
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// Metadata from this exact validated native view, without serializing its
    /// potentially large message body. Callers must not infer a different scope.
    pub fn metadata(&self) -> &Value {
        &self.view.meta
    }

    /// Native identity proven by the records of this exact view.
    pub fn native_scope(&self) -> Result<NativeScope, SessionError> {
        self.view.native_scope.clone()
    }

    pub fn messages(&self, query: &MessageQuery) -> Result<Value, SessionError> {
        validate_message_query(query)?;
        message_batch(self, query, None, None, None)
    }

    /// Same append selection as messages, without serializing message bodies or
    /// issuing media/file grants for conversations the browser has not opened.
    pub(crate) fn unread_summary(&self, query: &MessageQuery) -> Result<Value, SessionError> {
        validate_message_query(query)?;
        let selection = select_batch(self, query, None)?;
        let incoming = selection
            .selected
            .iter()
            .filter(|selected| {
                let message = &selected.event.message;
                message["counted"] != false
                    && matches!(
                        message["role"].as_str(),
                        Some(
                            "assistant"
                                | "assistant·subagent"
                                | "thinking"
                                | "tool"
                                | "tool_result"
                                | "question"
                        )
                    )
            })
            .count();
        Ok(json!({"reset": selection.reset, "start": selection.start,
            "end": self.view.parsed.committed, "version": {"head": self.head},
            "anchor": self.anchor, "incoming": incoming}))
    }

    /// The HTTP/SSE batch as bytes: the same document `messages_with_pages`
    /// produces, with every cacheable message spliced from the view's
    /// retained serialization instead of cloned and re-serialized
    /// (docs/read-model.md "视图字节缓存"). Run on the bounded reader.
    pub(crate) fn messages_body(
        &self,
        query: &MessageQuery,
        media: &crate::media::MediaStore,
        files: Option<&crate::files::FileService>,
        pages: &PageStore,
    ) -> Result<MessageBody, SessionError> {
        validate_message_query(query)?;
        let selection = select_batch(self, query, Some(pages))?;
        body::message_body(self, selection, media, files, pages)
    }

    pub(crate) fn valid_checkpoint(&self, query: &MessageQuery) -> bool {
        let start = usize::try_from(query.start).unwrap_or(usize::MAX);
        let parsed = &self.view.parsed;
        start <= parsed.committed
            && parsed.raw_index.is_checkpoint(query.start)
            && if start == parsed.committed {
                query.head == self.head && query.anchor == self.anchor
            } else {
                query.head == parsed.head(start) && query.anchor == anchor(&self.view, start)
            }
    }

    pub(crate) fn media_scope<'a>(
        &'a self,
        files: &'a crate::files::FileService,
    ) -> Result<crate::files::ScopedFiles<'a>, crate::files::FileError> {
        let meta = &self.view.meta;
        let refs = self
            .view
            .events()
            .flat_map(|event| {
                event
                    .media
                    .iter()
                    .filter_map(crate::media::NativeImage::file_ref)
            })
            .collect::<Vec<_>>();
        files.scoped_media_iter(
            meta["uid"].as_str().unwrap_or(""),
            meta["agent_id"].as_str().filter(|id| !id.is_empty()),
            meta["cwd"].as_str().unwrap_or(""),
            self.view.events().map(|event| &event.message),
            &refs,
            Some(self.revision()),
        )
    }

    pub fn cursor(&self) -> Value {
        json!({"end": self.view.parsed.committed, "head": self.head, "anchor": self.anchor})
    }

    /// Searchable semantic body: `(role, text)` of every non-status event
    /// in timeline order, without media, cursors or private payloads. The
    /// inferred rename event is not searchable.
    pub fn texts(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.view.texts()
    }

    /// The candidate (with the stamp that produced this view) a native span
    /// belongs to: membership in the full current branch first, then the
    /// exact source file. Never an external root or a fresh replacement stamp.
    pub(crate) fn native_source(
        &self,
        span: &crate::media::NativeSpan,
    ) -> Result<&Candidate, SessionError> {
        if !self
            .view
            .events()
            .flat_map(|event| &event.media)
            .any(|image| image.native_span() == Some(span))
        {
            return Err(SessionError::new(
                409,
                "图片已不属于当前会话分支，请重新加载会话",
            ));
        }
        self.view
            .sources
            .iter()
            .find(|candidate| candidate.root == span.root && candidate.data == span.path)
            .ok_or_else(|| SessionError::new(403, "图片缺少当前原生来源授权"))
    }
}

pub(crate) fn validate_message_query(query: &MessageQuery) -> Result<(), SessionError> {
    if !["", "0", "1"].contains(&query.append.as_str())
        || !["", "0", "1"].contains(&query.window.as_str())
    {
        return Err(SessionError::new(400, "append 和 window 只接受 0 或 1"));
    }
    Ok(())
}

/// Re-read exactly the frozen committed prefix of a parsed entry with the same
/// checked reader used for inherited prefixes; a byte mismatch is a retry, not
/// permission to validate against a different version than the one displayed.
pub(crate) fn committed_records(parsed: &Parsed) -> Result<Vec<(Value, u64)>, SessionError> {
    let candidate = &parsed.candidate;
    let Some(expected) = candidate.data_stamp() else {
        return Ok(Vec::new());
    };
    let cut = parsed.committed as u64;
    let mut reader =
        native_input::CheckedNative::open_prefix(&candidate.root, &candidate.data, expected, cut)?;
    let mut decoder = records::Decoder::cold();
    let index = records::scan_native_records(&mut reader, &mut decoder, None, candidate)?;
    reader.finish()?;
    if index.committed() != cut
        || index.prefix_hash(cut) != Some(parsed.prefix_hash(parsed.committed))
    {
        return Err(SessionError::new(503, "会话在校验期间变化，请重试"));
    }
    let batch = decoder.finish(parsed.committed);
    if let Some(error) = batch.error {
        return Err(SessionError::new(501, error));
    }
    Ok(batch.records)
}

/// Validate an operator-chosen Claude node against this parsed main session
/// and return the tip to pin plus the boundary it applies to. Reads native
/// bytes only; persisting the pin is the caller's step, and nothing here
/// signals the CLI.
pub(crate) fn claude_rewind_target(
    parsed: &Parsed,
    target: &str,
) -> Result<RewindTarget, SessionError> {
    if parsed.candidate.source != "claude" || !claude_agent_of(&parsed.candidate).is_empty() {
        return Err(SessionError::new(400, "这不是 Claude 主会话"));
    }
    if let Some(error) = parsed.raw_error.as_ref().or(parsed.unsupported.as_ref()) {
        return Err(SessionError::new(501, error.clone()));
    }
    let records = committed_records(parsed)?;
    let tip = providers::claude_pin_target(&records, target).map_err(|error| match error {
        providers::PinTargetError::Unknown => {
            SessionError::new(404, "目标不是这个 Claude 会话的记录节点")
        }
        providers::PinTargetError::Inactive => {
            SessionError::new(409, "目标不在当前 Claude 时间线上，无法固定显示")
        }
        providers::PinTargetError::NoParent => {
            SessionError::new(409, "首条消息之前没有可固定显示的时间线")
        }
        providers::PinTargetError::Unsupported(reason) => SessionError::new(501, reason),
    })?;
    Ok(RewindTarget {
        tip,
        stale_end: parsed.committed as u64,
    })
}

fn read_bounded_limit(
    root: &std::path::Path,
    path: &std::path::Path,
    expected: &super::FileStamp,
) -> Result<Vec<u8>, SessionError> {
    use std::io::Read;
    let mut file = native_input::CheckedNative::open(root, path, expected)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| SessionError::new(503, "会话文件读取失败"))?;
    file.finish()?;
    Ok(bytes)
}

pub(crate) fn read_native_input(
    candidate: &Candidate,
    decoder: &mut records::Decoder,
    probe: Option<u64>,
) -> Result<native_input::RawIndex, SessionError> {
    let Some(expected) = candidate.data_stamp() else {
        return records::scan_records(&b""[..], decoder, probe);
    };
    let mut reader = native_input::CheckedNative::open(&candidate.root, &candidate.data, expected)?;
    let index = records::scan_native_records(&mut reader, decoder, probe, candidate)?;
    reader.finish()?;
    Ok(index)
}

/// Stream ONE candidate file into an immutable [`Parsed`]. `previous` is the
/// last parse of the same file (same stamp lineage) whose ASTs may be reused
/// after full old-prefix digest verification; `cache` retains the resulting
/// ASTs for the next extension. The candidate's stamps are verified again
/// after reading: a change during the read is 503, never a partial snapshot.
pub(crate) fn parse_candidate(
    candidate: Candidate,
    previous: Option<&Parsed>,
    cache: &mut records::RecordCache,
    pin: Option<TimelinePin>,
) -> Result<Parsed, SessionError> {
    parse_candidate_retaining(candidate, previous, cache, pin, true)
}

/// `parse_candidate`, encoding details/checkpoints only when `retain`.
/// Otherwise the parse is search-only and must never back a ViewSnapshot.
pub(crate) fn parse_candidate_retaining(
    candidate: Candidate,
    previous: Option<&Parsed>,
    cache: &mut records::RecordCache,
    pin: Option<TimelinePin>,
    retain: bool,
) -> Result<Parsed, SessionError> {
    let (record_batch, raw_index) =
        cache.decode_input(&candidate, previous, |decoder, probe| {
            read_native_input(&candidate, decoder, probe)
        })?;
    let summary = candidate
        .summary
        .as_ref()
        .map(|path| {
            let bytes = read_bounded_limit(
                &candidate.root,
                path,
                candidate.summary_stamp().expect("summary stamp"),
            )?;
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|_| SessionError::new(503, "会话或子代理元数据尚不是完整有效的 JSON"))
        })
        .transpose()?;
    if candidate.source == "grok" {
        providers::validate_grok_summary(summary.as_ref().expect("Grok summary"))
            .map_err(|reason| SessionError::new(503, reason))?;
    }
    if restamp(&candidate)? != candidate {
        return Err(SessionError::new(503, "会话在读取期间变化，请重试"));
    }
    let committed = raw_index.committed() as usize;
    let records = &record_batch.records;
    let mut unsupported = record_batch.error.clone();
    let uid = uid_for(candidate.source, &candidate.path);
    // Grok falls back to chat mtime, or summary mtime if chat is absent;
    // this is metadata fallback only, never a fictional chat file version.
    let fallback = timestamp(
        candidate
            .data_stamp()
            .or_else(|| candidate.summary_stamp())
            .expect("candidate stamp")
            .modified,
    );
    let agent = claude_agent_of(&candidate);
    // A pin only ever changes the pure Claude parse options; the native file
    // and the CLI are untouched, and supersession by later native records is
    // reported in `timeline_pin`, never applied silently.
    let pin = pin.filter(|_| candidate.source == "claude" && agent.is_empty());
    let outcome = pin
        .as_ref()
        .map(|pin| providers::claude_pin(records, &pin.tip, pin.stale_end));
    let options = providers::ParseOptions {
        agent: &agent,
        declared_tip: outcome
            .as_ref()
            .and_then(|outcome| outcome.declared_tip.as_deref()),
        abandoned_after: outcome
            .as_ref()
            .map_or(0, |outcome| outcome.abandoned_after),
        // Lines the scanner skipped are a whole-file note the
        // projection cannot see in `records`.
        invalid_lines: record_batch.invalid,
    };
    let (mut meta, events, provider_error, append_projection) = if candidate.source == "agy"
        && record_batch.sidecars.is_empty()
        && options.agent.is_empty()
        && options.declared_tip.is_none()
        && options.abandoned_after == 0
    {
        // A probe survives decode_input only if an exact published AST entry
        // was reused and the entire committed prefix matched. A failed probe
        // is discarded by RecordCache's cold restart; no new fingerprint or
        // sampled-header evidence is needed here. Metadata/options changes
        // conservatively rebuild even when the native bytes did not change.
        let prefix = previous
            .filter(|previous| {
                previous.unsupported.is_none()
                    && previous.raw_error.is_none()
                    && previous.pin == pin
                    && previous.candidate.summary_stamp() == candidate.summary_stamp()
                    && raw_index.probe_digest() == Some(previous.raw_index.committed_digest())
            })
            .and_then(|previous| {
                previous
                    .append_projection
                    .as_ref()
                    .map(|state| providers::ProjectionPrefix {
                        state,
                        events: &previous.events,
                        committed: previous.committed as u64,
                    })
            });
        providers::parse_agy_append(records, summary.as_ref(), &fallback, options, prefix)
    } else {
        let (meta, events, error) = providers::parse_options_with_media(
            candidate.source,
            &candidate.path,
            records,
            summary.as_ref(),
            &fallback,
            options,
            &record_batch.sidecars,
        );
        (meta, events, error, None)
    };
    if let (Some(pin), Some(outcome)) = (&pin, &outcome) {
        meta["timeline_pin"] = json!({
            "target": pin.target, "tip": pin.tip, "stale_end": pin.stale_end,
            "pinned_at": pin.pinned_at, "retired": outcome.retired.is_some(),
            "native_rewind": false,
        });
        if pin.cli {
            meta["timeline_pin"]["cli"] = json!(true);
        }
        if let Some(retired) = outcome.retired {
            meta["timeline_pin"]["retired_reason"] = json!(retired.code());
            meta["timeline_pin"]["retired_message"] = json!(retired.message());
        }
    }
    let raw_error = unsupported.clone();
    if unsupported.is_none() {
        unsupported = provider_error
    }
    meta["uid"] = json!(uid);
    meta["source"] = json!(candidate.source);
    meta["path"] = json!(super::path_text(&candidate.path));
    meta["size"] = json!(raw_index.length());
    if candidate.source == "grok" {
        // The whole session directory, like the row.
        meta["size"] = json!(
            super::index::directory_size(&candidate.root, &candidate.path)
                .unwrap_or_else(|| candidate.stamps.iter().map(|stamp| stamp.size).sum::<u64>())
        );
        meta["chat_exists"] = json!(candidate.data_stamp().is_some());
    }
    meta["supported"] = json!(unsupported.is_none());
    // A hard failure is the only warning of an unsupported row; a supported row
    // keeps the provider's non-fatal notes (unknown kinds skipped).
    meta["migration_warnings"] = match &unsupported {
        Some(reason) => json!([reason]),
        None => meta["migration_warnings"]
            .as_array()
            .cloned()
            .map(Value::Array)
            .unwrap_or_else(|| json!([])),
    };
    // One serialization pass: the retained hot-read bytes, the committed
    // semantic digest and the LRU accounting. A previous parse of the same
    // file (an append) lends the bytes of every message it left unchanged.
    // Cold search consumes semantic text only. Serializing large tool output,
    // discovering its image references and hashing detail checkpoints would
    // scan it again despite none of it being searchable. Such parses stay
    // private to search_text and never become a ViewSnapshot or cached file.
    let encoded = if retain {
        encode_events(
            &events,
            true,
            previous.map(|previous| encoded::Previous {
                events: &previous.events,
                encoded: &previous.encoded,
            }),
        )?
    } else {
        EncodedEvents::default()
    };
    let (native_id, _declared) = if candidate.source == "grok" {
        scope::grok_native_identity(summary.as_ref())
    } else if matches!(candidate.source, "opencode" | "agy") {
        scope::summary_native_identity(summary.as_ref().map(|summary| &summary["session"]["id"]))
    } else {
        scope::native_identity(candidate.source, records)
    };
    cache.retain(candidate.clone(), record_batch);
    let append_projection = if retain && unsupported.is_none() {
        append_projection
    } else {
        None
    };
    let mut parsed = Parsed {
        native_id,
        candidate,
        raw_index,
        committed,
        meta,
        events,
        encoded,
        unsupported,
        raw_error,
        pin,
        append_projection,
    };
    if retain {
        parsed.meta["cursor"] = json!({
            "end": committed, "head": parsed.head(committed),
            "anchor": semantic_anchor(&history::native_identity(&parsed, &agent), &parsed, committed),
        });
    }
    Ok(parsed)
}

pub(crate) fn head(bytes: &[u8], end: usize) -> String {
    format!("{CURSOR_SCHEMA}:{}", &hash(&bytes[..end.min(4096)])[..16])
}

fn anchor(view: &View, end: usize) -> String {
    // Fixed inherited event meaning is bound by view.identity. Hashing it again
    // per child would duplicate work and make cheap sidebar cursors impossible.
    semantic_anchor(&view.identity, &view.parsed, end)
}

pub(crate) fn semantic_anchor(identity: &str, parsed: &Parsed, end: usize) -> String {
    // Appending native bytes can REMOVE or amend earlier visible messages.
    // Include the current semantic projection of the consumed prefix, plus
    // fixed inherited history and the selected view identity. Plain appends
    // leave this prefix unchanged; branch switches/rewrites force a reset.
    let projection = if end == parsed.committed {
        parsed.semantic_digest().to_owned()
    } else {
        parsed
            .encoded
            .digest_upto(&parsed.events, end as u64)
            .unwrap_or_else(|| projection_digest(&parsed.events, end))
    };
    let mut identity_parts = json!([
        CURSOR_SCHEMA,
        identity,
        end,
        parsed.prefix_hash(end),
        projection
    ]);
    // A persisted display pin (or its retirement) is a logical-view change
    // even when the consumed prefix bytes and its projection look the same;
    // a stale checkpoint must reset, never diff. Unpinned anchors are unchanged.
    if parsed.pin.is_some() {
        // Stable fields only: the float `pinned_at` and object insertion order
        // must not leak into the cursor identity, or a restart could reset a
        // view whose logical content did not change.
        let pin = &parsed.meta["timeline_pin"];
        identity_parts
            .as_array_mut()
            .expect("array above")
            .push(json!([
                pin["tip"].as_str().unwrap_or(""),
                pin["target"].as_str().unwrap_or(""),
                pin["stale_end"].as_u64().unwrap_or(0),
                pin["retired"].as_bool().unwrap_or(false),
                pin["retired_reason"].as_str().unwrap_or(""),
            ]));
    }
    hash(&serde_json::to_vec(&identity_parts).expect("primitive cursor identity"))
}

pub(crate) fn projection_digest(events: &[Event], end: usize) -> String {
    let mut digest = Sha1::new();
    for event in events {
        if event.end <= end as u64 {
            digest.update(event.end.to_le_bytes());
            let encoded = serde_json::to_vec(&event.message).expect("JSON values serialize");
            digest.update((encoded.len() as u64).to_le_bytes());
            digest.update(encoded);
            for image in &event.media {
                digest.update(image.semantic_key().as_bytes());
            }
        }
    }
    format!("{:x}", digest.finalize())
}

/// One batch's selection: which events go out and the small per-request
/// fields around them. Shared by the `Value` and the byte renderers.
pub(crate) struct Selection<'a> {
    pub selected: Vec<Selected<'a>>,
    pub reset: bool,
    pub start: usize,
    pub window: bool,
    pub total: usize,
    pub partial: Value,
    pub activity: Value,
    pub activity_changed: bool,
}

fn select_batch<'a>(
    snapshot: &'a ViewSnapshot,
    query: &MessageQuery,
    pages: Option<&PageStore>,
) -> Result<Selection<'a>, SessionError> {
    let view = &snapshot.view;
    let parsed = &view.parsed;
    let start = usize::try_from(query.start).unwrap_or(usize::MAX);
    let valid = snapshot.valid_checkpoint(query);
    let reset = !valid;
    let begin = if reset { 0 } else { start };
    let append_reset = reset && query.append == "1";
    let mut selected = Vec::new();
    let mut activity = Value::Null;
    let mut activity_changed = false;
    if !append_reset {
        let mut index = 0;
        for event in view.events() {
            if event.message["role"] == "status" {
                if reset || event.end > begin as u64 {
                    activity = event.message.clone();
                    activity_changed = true;
                }
                continue;
            }
            if reset || event.end > begin as u64 {
                selected.push(Selected { index, event });
            }
            index += 1;
        }
    }
    let total = selected
        .iter()
        .filter(|selected| selected.event.message["counted"] != false)
        .count();
    let window = reset && query.window == "1";
    let partial = if window && let Some(pages) = pages {
        pages::window(snapshot, &mut selected, pages)?
    } else if window && selected.len() > 600 {
        let omitted = selected.len() - 600;
        selected.drain(100..selected.len() - 500);
        json!({"head": 100, "tail": 500, "omitted": omitted})
    } else {
        Value::Null
    };
    Ok(Selection {
        selected,
        reset,
        start: if append_reset {
            parsed.committed
        } else {
            begin
        },
        window,
        total,
        partial,
        activity,
        activity_changed,
    })
}

/// The per-request fields before `messages` (`meta` … `anchor`) and after
/// it (`message_total` … `activity`), exactly as `message_batch` orders them.
pub(crate) fn batch_fields(snapshot: &ViewSnapshot, selection: &Selection<'_>) -> (Value, Value) {
    let view = &snapshot.view;
    let parsed = &view.parsed;
    let current_anchor = &snapshot.anchor;
    let mut meta = view.meta.clone();
    meta["cursor"] = json!({"end": parsed.committed,
        "head": parsed.head(parsed.committed), "anchor": current_anchor});
    let head = json!({
        "meta": meta,
        "version": {"size": parsed.raw_index.length(),
                    "exists": parsed.candidate.data_stamp().is_some(),
                    "mtime": parsed.candidate.data_stamp().map(|stamp| stamp.modified / 1_000_000),
                    "head": parsed.head(parsed.committed)},
        "reset": selection.reset, "start": selection.start,
        "end": parsed.committed, "anchor": current_anchor,
    });
    let tail = json!({
        "message_total": selection.total, "partial": selection.partial,
        "activity_changed": selection.activity_changed, "activity": selection.activity,
    });
    (head, tail)
}

fn message_batch(
    snapshot: &ViewSnapshot,
    query: &MessageQuery,
    media: Option<&crate::media::MediaStore>,
    files: Option<&crate::files::FileService>,
    pages: Option<&PageStore>,
) -> Result<Value, SessionError> {
    let selection = select_batch(snapshot, query, pages)?;
    let messages = project_selected(snapshot, &selection.selected, media, files, pages)?;
    let (mut response, tail) = batch_fields(snapshot, &selection);
    response["messages"] = Value::Array(messages);
    for (key, value) in tail.as_object().expect("object above") {
        response[key] = value.clone();
    }
    if pages.is_some() && selection.window {
        pages::validate_response(&response)?;
    }
    Ok(response)
}

/// Public message projection. With a media store, each message carries at most
/// `DISPLAY_LIMIT` typed images inline plus `media_more` for any remainder;
/// text-only consumers (search, input history) never see either field.
pub(crate) fn project_selected(
    snapshot: &ViewSnapshot,
    selected: &[Selected<'_>],
    media: Option<&crate::media::MediaStore>,
    files: Option<&crate::files::FileService>,
    pages: Option<&PageStore>,
) -> Result<Vec<Value>, SessionError> {
    let projected = if let Some(media) = media {
        let events = selected
            .iter()
            .map(|selected| selected.event)
            .collect::<Vec<_>>();
        media_projection::project(snapshot, &events, media, files)?
    } else {
        Vec::new()
    };
    let mut messages = Vec::with_capacity(selected.len());
    for (index, selected) in selected.iter().enumerate() {
        let mut message = selected.event.message.clone();
        if media.is_some() {
            if !projected[index].is_empty() {
                message["media"] = json!(&projected[index]);
            }
            if let Some(more) = pages::media_more(snapshot, selected, pages)? {
                message["media_more"] = more;
            }
        }
        messages.push(message);
    }
    Ok(messages)
}

// ---------------------------------------------------------------------------
// On-demand per-session view cache.
// ---------------------------------------------------------------------------

/// Everything `Views` needs to build ONE logical view, as the index knows it.
/// Candidates carry the index's last stamps; `Views` restamps them itself and
/// treats any difference as an append/rewrite of that one file.
#[derive(Clone)]
pub(crate) struct ViewRequest {
    /// Canonical owner UID (`source:sha1(path)[:16]` of the main session).
    pub uid: String,
    /// Exact agent id (`""` for the main transcript); the id must match an
    /// entry of `row["agent_items"]` when `selected` is given.
    pub agent: String,
    /// The owner's (main session's) candidate file.
    pub owner: Candidate,
    /// The agent's own file (Claude `agent-<id>.jsonl` sidecar, Codex subagent
    /// rollout) when `agent` is not empty; `None` selects the owner itself.
    pub selected: Option<Candidate>,
    /// Persisted Claude display pin of the owner (main sessions only).
    pub pin: Option<TimelinePin>,
    /// The owner's published list row (topology, names and metadata already
    /// applied by the index); `messages.meta` is this row, so the
    /// view's metadata comes from here, never from a second derivation.
    pub row: Value,
}

/// How `Views` obtains the files a view depends on beyond the request:
/// inherited Codex fixed prefixes name their parent by native thread id.
/// Implementations resolve against the index; `Views` never joins paths.
pub(crate) trait Dependencies {
    /// The unique main (non-subagent) `source` transcript whose native thread
    /// id is `thread_id`. Errors use the established retry/contract codes:
    /// 501 when the parent is not indexed or is a subagent file, 409 when the
    /// id is ambiguous.
    fn thread(&self, source: &str, thread_id: &str) -> Result<Candidate, SessionError>;
    fn thread_from(
        &self,
        source: &str,
        thread_id: &str,
        _child: &str,
        _base: &Value,
    ) -> Result<Candidate, SessionError> {
        self.thread(source, thread_id)
    }
    /// A parse of exactly this candidate (same stamps) and pin the caller
    /// already holds, so one file is never streamed twice for one change.
    /// Optional; the lazy index has none.
    fn parsed(&self, _candidate: &Candidate, _pin: Option<&TimelinePin>) -> Option<Arc<Parsed>> {
        None
    }
}

#[derive(Clone)]
struct FileEntry {
    parsed: Arc<Parsed>,
    encoded: usize,
    used: Instant,
}

/// One inherited fixed prefix, with the stamp of the parent file it was read
/// from: an unchanged stamp lets a rebuild of the child skip re-reading it.
#[derive(Clone)]
struct Prefix {
    child: String,
    base: Value,
    /// The thread id the child declared for this parent.
    thread: String,
    candidate: Candidate,
    cut: usize,
    digest: Value,
}

#[derive(Clone)]
struct CachedView {
    snapshot: Arc<ViewSnapshot>,
    owner: Option<Arc<Parsed>>,
    prefixes: Vec<Prefix>,
    pin: Option<TimelinePin>,
    inherited_encoded: usize,
    used: Instant,
}

impl CachedView {
    /// Still describes exactly these files: leaf/owner stamps and pin
    /// unchanged, and every inherited parent still resolves (through the
    /// index) to the same file with the same stamp. A parent that became
    /// ambiguous or vanished surfaces its own error here, not a stale view.
    fn is_current(
        &self,
        leaf: &Candidate,
        owner: &Candidate,
        pin: Option<&TimelinePin>,
        deps: &dyn Dependencies,
    ) -> Result<bool, SessionError> {
        if self.snapshot.view.parsed.candidate != *leaf
            || self.pin.as_ref() != pin
            || self
                .owner
                .as_ref()
                .is_some_and(|parsed| parsed.candidate != *owner)
        {
            return Ok(false);
        }
        for prefix in &self.prefixes {
            let resolved = deps.thread_from(
                prefix.candidate.source,
                &prefix.thread,
                &prefix.child,
                &prefix.base,
            )?;
            if resolved.path != prefix.candidate.path
                || restamp(&prefix.candidate)? != prefix.candidate
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// One projected inherited prefix `[0, cut)` of a parent file (events with
/// physical end zero), shared by every view and transient search projection
/// that declares it: ten forks of one gigabyte parent read it once, not ten
/// times. An entry is valid only for exactly the stamp it was read with.
struct PrefixEntry {
    candidate: Candidate,
    meta: Value,
    events: Arc<Vec<Event>>,
    digest: String,
    encoded: usize,
    used: Instant,
}

/// Bounded LRU of inherited prefixes keyed by (parent uid, cut).
#[derive(Default)]
pub(crate) struct PrefixCache {
    entries: BTreeMap<(String, usize), PrefixEntry>,
    eviction: u64,
    view_bytes: usize,
}

/// Retained inherited prefixes: a handful of parents, within the view byte
/// budget (serialized event bytes, like the view LRU).
const PREFIX_ENTRIES: usize = 8;

impl PrefixCache {
    fn evict(&mut self, uid: &str, retired: &mut Retired) {
        self.eviction = self.eviction.wrapping_add(1);
        let keys = self
            .entries
            .keys()
            .filter(|(parent, _)| parent == uid)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            retired
                .prefixes
                .push(self.entries.remove(&key).expect("selected prefix"));
        }
    }

    fn bytes(&self) -> usize {
        self.entries
            .values()
            .map(|entry| entry.encoded)
            .fold(0, usize::saturating_add)
    }

    fn reserve_views(&mut self, bytes: usize, retired: &mut Retired) {
        self.view_bytes = bytes;
        while self.bytes().saturating_add(bytes) > view_byte_limit() {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            retired
                .prefixes
                .push(self.entries.remove(&oldest).expect("selected prefix"));
        }
    }

    fn get(
        &mut self,
        candidate: &Candidate,
        cut: usize,
    ) -> Option<(Value, Arc<Vec<Event>>, String)> {
        let key = (uid_for(candidate.source, &candidate.path), cut);
        let entry = self.entries.get_mut(&key)?;
        if entry.candidate != *candidate {
            return None;
        }
        entry.used = Instant::now();
        Some((
            entry.meta.clone(),
            entry.events.clone(),
            entry.digest.clone(),
        ))
    }
    fn insert(&mut self, cut: usize, entry: PrefixEntry, retired: &mut Retired) {
        let encoded = entry.encoded;
        let limit = view_byte_limit().saturating_sub(self.view_bytes);
        if encoded > limit {
            retired.prefixes.push(entry);
            return;
        }
        let bytes = |entries: &BTreeMap<(String, usize), PrefixEntry>| {
            entries
                .values()
                .map(|entry| entry.encoded)
                .fold(0usize, usize::saturating_add)
        };
        while !self.entries.is_empty()
            && (self.entries.len() >= PREFIX_ENTRIES
                || bytes(&self.entries).saturating_add(encoded) > limit)
        {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
                .expect("nonempty");
            retired
                .prefixes
                .push(self.entries.remove(&oldest).expect("selected prefix"));
        }
        let key = (uid_for(entry.candidate.source, &entry.candidate.path), cut);
        if let Some(old) = self.entries.insert(key, entry) {
            retired.prefixes.push(old);
        }
    }
}

/// The cached view with the request's row: the same snapshot when the
/// published row is unchanged, otherwise the same immutable parts under a
/// new meta (star, rename, agent menu) without touching any file.
fn recompose(
    request: &ViewRequest,
    snapshot: &Arc<ViewSnapshot>,
) -> Result<Arc<ViewSnapshot>, SessionError> {
    let old = &snapshot.view;
    let meta = view_meta(request, &old.parsed)?;
    if old.meta == meta {
        return Ok(snapshot.clone());
    }
    Ok(Arc::new(ViewSnapshot::new(Arc::new(View::new(
        ViewParts {
            parsed: old.parsed.clone(),
            meta,
            inherited: old.inherited.clone(),
            inherited_encoded: old.inherited_encoded.clone(),
            identity: old.identity.clone(),
            native_scope: old.native_scope.clone(),
            sources: old.sources.clone(),
            dependencies: old.dependencies.clone(),
        },
    )))))
}

/// Bounded LRU of opened sessions. The list never opens through it; it
/// only borrows what a cached view already knows (`view_decorations`).
#[derive(Default)]
pub(crate) struct Views {
    files: BTreeMap<String, FileEntry>,
    views: BTreeMap<(String, String), CachedView>,
    records: records::RecordCache,
    /// Shared with the store's transient (search) projections.
    prefixes: Arc<Mutex<PrefixCache>>,
    /// Bumped whenever a `(uid, agent)` entry is inserted, replaced by a
    /// different snapshot or removed — exactly the changes that can alter
    /// what the list borrows from this cache. Shared with the facade
    /// (`revision_handle`) so the list's serialized-bytes cache can compare
    /// it without taking this lock.
    revision: Arc<AtomicU64>,
    /// Only active opens have a gate. No idle per-UID cache or lock table.
    flights: BTreeMap<String, Weak<Flight>>,
    /// An eviction prevents overlapping builds from repopulating the cache,
    /// including dependencies not yet resolved when the eviction occurred.
    eviction: u64,
}

/// Which pin a parsed file must carry: the leaf's exact display pin, or any
/// (an owner consulted only for its native identity).
#[derive(Clone, Copy)]
enum Pin<'a> {
    Exact(Option<&'a TimelinePin>),
    Any,
}

/// Where a parsed leaf/owner comes from during one build, with its
/// serialized-message size (computed once per parse).
trait FileSource {
    fn file(
        &mut self,
        candidate: Candidate,
        pin: Pin<'_>,
        deps: &dyn Dependencies,
    ) -> Result<(Arc<Parsed>, usize), SessionError>;
    /// Whether this build keeps serialized message bytes for hot reads.
    fn retains(&self) -> bool;
}

impl Views {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The inherited-prefix cache, to share with `search_text`.
    pub(crate) fn prefixes(&self) -> Arc<Mutex<PrefixCache>> {
        self.prefixes.clone()
    }

    /// The counter behind [`Views::revision`], readable without the lock.
    pub(crate) fn revision_handle(&self) -> Arc<AtomicU64> {
        self.revision.clone()
    }

    /// The current revision of the cached-view set (see the field).
    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    fn bump(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    fn byte_limit(&self) -> usize {
        view_byte_limit()
    }

    /// File I/O, version checks, metadata recomposition and provider work
    /// run outside the shared cache lock. The lease serializes only opens of
    /// this owner; a dependency never acquires another owner's gate.
    pub(crate) fn open<'a>(
        shared: &'a Mutex<Self>,
        request: &ViewRequest,
        deps: &dyn Dependencies,
    ) -> Result<PendingView<'a>, SessionError> {
        validate_request(request)?;
        let lease = OpenLease::acquire(shared, &request.uid)?;
        let key = (request.uid.clone(), request.agent.clone());
        let (previous, prefixes, eviction) = {
            let cache = lock_views(shared)?;
            (
                cache.views.get(&key).cloned(),
                cache.prefixes.clone(),
                cache.eviction,
            )
        };
        let owner = restamp(&request.owner)?;
        let selected = request.selected.as_ref().map(restamp).transpose()?;
        let leaf = selected.clone().unwrap_or_else(|| owner.clone());
        let pin = leaf_pin(request, &leaf);
        let mut files = CachedFiles {
            shared,
            eviction,
            files: BTreeMap::new(),
            records: Vec::new(),
            fresh: false,
        };
        let cached = if let Some(cached) = previous.as_ref()
            && cached.is_current(&leaf, &owner, pin.as_ref(), deps)?
        {
            let mut cached = cached.clone();
            cached.snapshot = recompose(request, &cached.snapshot)?;
            cached.used = Instant::now();
            cached
        } else {
            let built = build(
                request,
                deps,
                &mut files,
                Some(&prefixes),
                owner,
                selected,
                pin.clone(),
                previous.as_ref().map(|cached| cached.prefixes.as_slice()),
                previous.as_ref().map(|cached| {
                    (
                        cached.snapshot.view.inherited.clone(),
                        cached.snapshot.view.inherited_encoded.clone(),
                    )
                }),
                true,
            )?;
            CachedView {
                snapshot: Arc::new(ViewSnapshot::new(Arc::new(built.view))),
                owner: built.owner,
                prefixes: built.prefixes,
                pin,
                inherited_encoded: built.inherited_encoded,
                used: Instant::now(),
            }
        };
        Ok(PendingView {
            retired: Retired::default(),
            lease,
            key,
            cached,
            files,
        })
    }

    /// Clone a bounded cache entry under lock, then stat/resolve/recompose
    /// without blocking independent opens or list decorations.
    pub(crate) fn cached_current(
        shared: &Mutex<Self>,
        request: &ViewRequest,
        deps: &dyn Dependencies,
    ) -> Result<Option<Arc<ViewSnapshot>>, SessionError> {
        validate_request(request)?;
        let key = (request.uid.clone(), request.agent.clone());
        let (cached, eviction) = {
            let cache = lock_views(shared)?;
            (cache.views.get(&key).cloned(), cache.eviction)
        };
        let Some(cached) = cached else {
            return Ok(None);
        };
        let owner = restamp(&request.owner)?;
        let leaf = request
            .selected
            .as_ref()
            .map(restamp)
            .transpose()?
            .unwrap_or_else(|| owner.clone());
        if !cached.is_current(&leaf, &owner, leaf_pin(request, &leaf).as_ref(), deps)? {
            return Ok(None);
        }
        let snapshot = recompose(request, &cached.snapshot)?;
        let mut cache = lock_views(shared)?;
        if cache.eviction != eviction {
            return Ok(None);
        }
        let Some(current) = cache.views.get_mut(&key) else {
            return Ok(None);
        };
        if !Arc::ptr_eq(&current.snapshot, &cached.snapshot) {
            return Ok(None);
        }
        current.used = Instant::now();
        Ok(Some(snapshot))
    }

    /// Drop every view of this owner UID and the parsed files behind them.
    pub(crate) fn evict(&mut self, uid: &str) -> Retired {
        let mut retired = Retired::default();
        self.eviction = self.eviction.wrapping_add(1);
        let keys = self
            .views
            .iter()
            .filter(|(key, cached)| {
                key.0 == uid
                    || cached
                        .snapshot
                        .view
                        .dependencies
                        .iter()
                        .any(|dependency| dependency == uid)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        if !keys.is_empty() {
            self.bump();
        }
        for key in keys {
            retired
                .views
                .push(self.views.remove(&key).expect("selected view"));
        }
        self.flights.remove(uid);
        retired.records.push(self.records.evict(uid));
        if let Ok(mut prefixes) = self.prefixes.lock() {
            prefixes.evict(uid, &mut retired);
        }
        if let Some(file) = self.files.remove(uid) {
            retired.files.push(file);
        }
        self.prune(&mut retired);
        retired
    }

    /// The bounded set of immutable snapshots borrowed by list rendering.
    /// Decorations and row cloning run after releasing the cache lock.
    pub(crate) fn decorations(&self) -> BTreeMap<(String, String), Arc<ViewSnapshot>> {
        self.views
            .iter()
            .map(|(key, cached)| (key.clone(), cached.snapshot.clone()))
            .collect()
    }

    fn bytes(&self) -> usize {
        self.files
            .values()
            .map(|entry| entry.encoded)
            .chain(self.views.values().map(|cached| cached.inherited_encoded))
            .fold(0, usize::saturating_add)
    }

    /// Views whose files were evicted or replaced are stale; drop them, then
    /// keep the view count within its bound (files are bounded on insert).
    fn prune(&mut self, retired: &mut Retired) {
        while self.files.len() > view_limit() || self.bytes() > self.byte_limit() {
            let Some(oldest) = self
                .files
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            retired
                .files
                .push(self.files.remove(&oldest).expect("selected file"));
        }
        let before = self.views.len();
        let files = &self.files;
        let stale = self
            .views
            .iter()
            .filter(|(_, cached)| {
                let current = |parsed: &Arc<Parsed>| {
                    files
                        .get(&uid_for(parsed.candidate.source, &parsed.candidate.path))
                        .is_some_and(|entry| Arc::ptr_eq(&entry.parsed, parsed))
                };
                !current(&cached.snapshot.view.parsed) || !cached.owner.as_ref().is_none_or(current)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in stale {
            retired
                .views
                .push(self.views.remove(&key).expect("selected view"));
        }
        while self.views.len() > view_limit() {
            let oldest = self
                .views
                .iter()
                .min_by_key(|(_, cached)| cached.used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            retired
                .views
                .push(self.views.remove(&oldest).expect("selected view"));
        }
        if self.views.len() != before {
            self.bump();
        }
        if let Ok(mut prefixes) = self.prefixes.lock() {
            prefixes.reserve_views(self.bytes(), retired);
        }
    }
}

/// Cache payloads unlinked under lock and destroyed after unlock.
#[derive(Default)]
pub(crate) struct Retired {
    files: Vec<FileEntry>,
    views: Vec<CachedView>,
    prefixes: Vec<PrefixEntry>,
    records: Vec<records::RecordCache>,
    asts: Vec<records::RetiredRecords>,
}

fn lock_views(shared: &Mutex<Views>) -> Result<MutexGuard<'_, Views>, SessionError> {
    shared
        .lock()
        .map_err(|_| SessionError::new(500, "会话视图锁不可用"))
}

#[derive(Default)]
struct Flight {
    busy: Mutex<bool>,
    ready: Condvar,
}

struct OpenLease<'a> {
    shared: &'a Mutex<Views>,
    uid: String,
    flight: Arc<Flight>,
}
impl<'a> OpenLease<'a> {
    fn acquire(shared: &'a Mutex<Views>, uid: &str) -> Result<Self, SessionError> {
        let flight = {
            let mut cache = lock_views(shared)?;
            match cache.flights.get(uid).and_then(Weak::upgrade) {
                Some(flight) => flight,
                None => {
                    let flight = Arc::new(Flight::default());
                    cache
                        .flights
                        .insert(uid.to_owned(), Arc::downgrade(&flight));
                    flight
                }
            }
        };
        let mut busy = flight
            .busy
            .lock()
            .map_err(|_| SessionError::new(500, "会话构建锁不可用"))?;
        while *busy {
            busy = flight
                .ready
                .wait(busy)
                .map_err(|_| SessionError::new(500, "会话构建锁不可用"))?;
        }
        *busy = true;
        drop(busy);
        Ok(Self {
            shared,
            uid: uid.to_owned(),
            flight,
        })
    }
}
impl Drop for OpenLease<'_> {
    fn drop(&mut self) {
        if let Ok(mut busy) = self.flight.busy.lock() {
            *busy = false;
            self.flight.ready.notify_all();
        }
        if let Ok(mut cache) = self.shared.lock()
            && Arc::strong_count(&self.flight) == 1
            && cache
                .flights
                .get(&self.uid)
                .is_some_and(|flight| Weak::ptr_eq(flight, &Arc::downgrade(&self.flight)))
        {
            cache.flights.remove(&self.uid);
        }
    }
}

/// A result awaiting the facade's current-index/pin check. Uncommitted work
/// owns its ASTs and parsed files; eviction or a failed build drops it all.
pub(crate) struct PendingView<'a> {
    retired: Retired,
    lease: OpenLease<'a>,
    key: (String, String),
    cached: CachedView,
    files: CachedFiles<'a>,
}
impl PendingView<'_> {
    pub(crate) fn refresh(
        &mut self,
        request: &ViewRequest,
        deps: &dyn Dependencies,
    ) -> Result<(), SessionError> {
        validate_request(request)?;
        let owner = restamp(&request.owner)?;
        let leaf = request
            .selected
            .as_ref()
            .map(restamp)
            .transpose()?
            .unwrap_or_else(|| owner.clone());
        if !self
            .cached
            .is_current(&leaf, &owner, leaf_pin(request, &leaf).as_ref(), deps)?
        {
            return Err(SessionError::new(503, "会话在读取期间变化，请重试"));
        }
        self.cached.snapshot = recompose(request, &self.cached.snapshot)?;
        Ok(())
    }

    /// Called only while the facade holds its short publication guard. No
    /// filesystem or provider work is performed here; displaced ASTs leave
    /// with PendingView and are freed after the shared locks are released.
    pub(crate) fn publish(&mut self, cache: &mut Views) -> Result<Arc<ViewSnapshot>, SessionError> {
        if !cache
            .flights
            .get(&self.key.0)
            .is_some_and(|flight| Weak::ptr_eq(flight, &Arc::downgrade(&self.lease.flight)))
        {
            return Err(SessionError::new(503, "会话视图已失效，请重试"));
        }
        if cache.eviction != self.files.eviction {
            // The facade just validated this snapshot against the latest
            // published owner, pin and dependency graph. An unrelated deletion
            // must not make that valid read fail. Return it without retaining
            // any overlapping work, so an evicted entry cannot come back.
            return Ok(self.cached.snapshot.clone());
        }
        let now = Instant::now();
        for (id, mut entry) in std::mem::take(&mut self.files.files) {
            entry.used = now;
            if let Some(old) = cache.files.insert(id, entry) {
                self.retired.files.push(old);
            }
        }
        // AST cache admission remains globally bounded, not per owner.
        for records in std::mem::take(&mut self.files.records) {
            self.retired.asts.push(cache.records.absorb(records));
        }
        let snapshot = self.cached.snapshot.clone();
        for id in &snapshot.view.dependencies {
            if let Some(entry) = cache.files.get_mut(id) {
                entry.used = now;
            }
        }
        let changed = cache
            .views
            .get(&self.key)
            .is_none_or(|old| !Arc::ptr_eq(&old.snapshot, &snapshot));
        self.cached.used = now;
        if let Some(old) = cache.views.insert(self.key.clone(), self.cached.clone()) {
            self.retired.views.push(old);
        }
        if changed {
            cache.bump();
        }
        cache.prune(&mut self.retired);
        Ok(snapshot)
    }
}

struct CachedFiles<'a> {
    shared: &'a Mutex<Views>,
    eviction: u64,
    files: BTreeMap<String, FileEntry>,
    records: Vec<records::RecordCache>,
    fresh: bool,
}
impl Drop for CachedFiles<'_> {
    fn drop(&mut self) {
        self.records.clear();
        self.files.clear();
        if self.fresh {
            crate::sessions::memory::release();
        }
    }
}
impl FileSource for CachedFiles<'_> {
    fn file(
        &mut self,
        candidate: Candidate,
        pin: Pin<'_>,
        deps: &dyn Dependencies,
    ) -> Result<(Arc<Parsed>, usize), SessionError> {
        let id = uid_for(candidate.source, &candidate.path);
        let matches = |entry: &FileEntry| {
            entry.parsed.candidate == candidate
                && match pin {
                    Pin::Exact(pin) => entry.parsed.pin.as_ref() == pin,
                    Pin::Any => true,
                }
        };
        if let Some(entry) = self.files.get(&id).filter(|entry| matches(entry)) {
            return Ok((entry.parsed.clone(), entry.encoded));
        }
        let (previous, mut records) = {
            let mut cache = lock_views(self.shared)?;
            let previous = cache.files.get(&id).cloned();
            if let Some(entry) = previous.as_ref().filter(|entry| matches(entry)) {
                self.files.insert(id, entry.clone());
                return Ok((entry.parsed.clone(), entry.encoded));
            }
            (previous, cache.records.checkout(&id))
        };
        let pin = match pin {
            Pin::Exact(pin) => pin,
            Pin::Any => None,
        };
        let parsed = match deps.parsed(&candidate, pin) {
            Some(parsed) => parsed,
            None => {
                self.fresh = true;
                Arc::new(parse_candidate(
                    candidate,
                    previous.as_ref().map(|entry| entry.parsed.as_ref()),
                    &mut records,
                    pin.cloned(),
                )?)
            }
        };
        let encoded = parsed.encoded_bytes();
        self.records.push(records);
        self.files.insert(
            id,
            FileEntry {
                parsed: parsed.clone(),
                encoded,
                used: Instant::now(),
            },
        );
        Ok((parsed, encoded))
    }
    fn retains(&self) -> bool {
        true
    }
}

/// Cold, unretained parses for one transient build (search misses).
struct ColdFiles {
    records: records::RecordCache,
}
impl FileSource for ColdFiles {
    fn file(
        &mut self,
        candidate: Candidate,
        pin: Pin<'_>,
        deps: &dyn Dependencies,
    ) -> Result<(Arc<Parsed>, usize), SessionError> {
        let pin = match pin {
            Pin::Exact(pin) => pin,
            Pin::Any => None,
        };
        let parsed = match deps.parsed(&candidate, pin) {
            Some(parsed) => parsed,
            None => Arc::new(parse_candidate_retaining(
                candidate,
                None,
                &mut self.records,
                pin.cloned(),
                false,
            )?),
        };
        // Nothing will extend this parse: free the decoded records now, not
        // when the whole transient open ends (search cold paths run several
        // of these at once; the AST is the bulk of a projection's memory).
        self.records = records::RecordCache::default();
        let encoded = parsed.encoded_bytes();
        Ok((parsed, encoded))
    }
    fn retains(&self) -> bool {
        false
    }
}

/// Extract one session's search body without constructing detail bytes or
/// checkpoints. The ASTs it decoded are dropped with this call; inherited prefixes go
/// through the shared prefix cache (when given), so forks of one parent do
/// not stream that parent once per search miss.
pub(crate) fn search_text(
    request: &ViewRequest,
    deps: &dyn Dependencies,
    prefixes: Option<&Mutex<PrefixCache>>,
) -> Result<String, SessionError> {
    validate_request(request)?;
    let owner = restamp(&request.owner)?;
    let selected = request.selected.as_ref().map(restamp).transpose()?;
    let leaf = selected.clone().unwrap_or_else(|| owner.clone());
    let pin = leaf_pin(request, &leaf);
    let mut files = ColdFiles {
        records: records::RecordCache::default(),
    };
    let built = build(
        request, deps, &mut files, prefixes, owner, selected, pin, None, None, false,
    )?;
    Ok(crate::search::body_texts(built.view.texts()))
}

fn validate_request(request: &ViewRequest) -> Result<(), SessionError> {
    if request.uid.is_empty() {
        return Err(SessionError::new(404, "会话不存在"));
    }
    if request.selected.is_some() != !request.agent.is_empty() {
        return Err(SessionError::new(404, "子代理不存在或不属于此主会话"));
    }
    if let Some(selected) = &request.selected
        && selected.source != request.owner.source
    {
        return Err(SessionError::new(409, "子代理与主会话的数据源不一致"));
    }
    Ok(())
}

/// The pin applies to the parsed leaf only when it is a Claude main session.
fn leaf_pin(request: &ViewRequest, leaf: &Candidate) -> Option<TimelinePin> {
    if request.agent.is_empty() && leaf.source == "claude" && claude_agent_of(leaf).is_empty() {
        request.pin.clone()
    } else {
        None
    }
}

/// View metadata from the published owner row: `session_view`
/// for an agent, the row itself for the main transcript. The
/// row's `migration_warnings` come from the head/tail summary; the detail
/// merges the projection's whole-file notes into them.
fn view_meta(request: &ViewRequest, parsed: &Parsed) -> Result<Value, SessionError> {
    let mut meta = request.row.clone();
    if !meta.is_object() {
        return Err(SessionError::new(500, "会话行不是对象"));
    }
    if request.agent.is_empty() {
        if parsed.meta["timeline_pin"].is_object() {
            meta["timeline_pin"] = parsed.meta["timeline_pin"].clone();
        }
        meta["migration_warnings"] = merged_warnings(&meta, parsed);
        return Ok(meta);
    }
    let item = meta["agent_items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["id"] == request.agent)
        .cloned()
        .ok_or_else(|| SessionError::new(404, "子代理不存在或不属于此主会话"))?;
    meta["parent_title"] = meta["title"].clone();
    meta["sid"] = item["id"].clone();
    meta["agent_id"] = item["id"].clone();
    meta["agent_type"] = item["type"].clone();
    meta["title"] = item["title"].clone();
    for field in [
        "path",
        "cwd",
        "model",
        "created",
        "updated",
        "size",
        "supported",
        "migration_warnings",
    ] {
        if let Some(value) = item.get(field) {
            meta[field] = value.clone()
        }
    }
    meta["migration_warnings"] = merged_warnings(&meta, parsed);
    let object = meta.as_object_mut().expect("metadata object");
    object.remove("cursor");
    // A display pin applies to the main transcript only, never to a
    // subagent view projected from its own sidecar file.
    object.remove("timeline_pin");
    Ok(meta)
}

/// The row's head/tail notes with the projection's whole-file notes merged
/// in: a note with the same label (text before ` ×`, or the overflow line)
/// takes the projection's exact count in place, everything else (lineage
/// truncation) is appended. Only a supported parse reaches here, so both
/// lists are notes, never a hard reason.
fn merged_warnings(meta: &Value, parsed: &Parsed) -> Value {
    fn label(note: &str) -> &str {
        if note.starts_with("另有 ") {
            return "另有";
        }
        note.rsplit_once(" ×").map_or(note, |(label, _)| label)
    }
    let mut merged = meta["migration_warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for note in parsed.meta["migration_warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        match merged.iter().position(|known| label(known) == label(note)) {
            Some(index) => merged[index] = note.to_owned(),
            None => merged.push(note.to_owned()),
        }
    }
    json!(merged)
}

struct Built {
    view: View,
    owner: Option<Arc<Parsed>>,
    prefixes: Vec<Prefix>,
    inherited_encoded: usize,
}

#[allow(clippy::too_many_arguments)]
fn build(
    request: &ViewRequest,
    deps: &dyn Dependencies,
    files: &mut dyn FileSource,
    prefix_cache: Option<&Mutex<PrefixCache>>,
    owner: Candidate,
    selected: Option<Candidate>,
    pin: Option<TimelinePin>,
    old_prefixes: Option<&[Prefix]>,
    old_inherited: Option<(Arc<Vec<Event>>, Arc<EncodedEvents>)>,
    include_native_scope: bool,
) -> Result<Built, SessionError> {
    let leaf = selected.clone().unwrap_or_else(|| owner.clone());
    let (parsed, _) = files.file(leaf, Pin::Exact(pin.as_ref()), deps)?;
    let owner_parsed = match selected {
        Some(_) if include_native_scope => Some(files.file(owner, Pin::Any, deps)?.0),
        _ => None,
    };
    if let Some(error) = parsed.raw_error.as_ref().or(parsed.unsupported.as_ref()) {
        return Err(SessionError::new(501, error.clone()));
    }
    // Search consumes only the leaf's semantic text and inherited prefixes.
    // Reading the full owner for every sidecar would multiply cold-search I/O;
    // transient search views are never used to authorize native operations.
    let native_scope = if include_native_scope {
        native_scope(request, owner_parsed.as_deref().unwrap_or(&parsed), &parsed)
    } else {
        Err(SessionError::new(501, "搜索投影不提供原生操作范围"))
    };
    // Inherited fixed prefixes: reuse the previous chain only when the leaf
    // still declares the same direct parent/cut and every parent file still
    // carries exactly the stamp it was read with (deepest ancestor first).
    let declared = history::history_link(parsed.candidate.source, &parsed.meta)?
        .map(|(sid, cut)| (sid.to_owned(), cut));
    let reusable = old_prefixes.filter(|prefixes| {
        declared == prefixes.last().map(|p| (p.thread.clone(), p.cut))
            && prefixes
                .iter()
                .all(|prefix| restamp(&prefix.candidate).is_ok_and(|now| now == prefix.candidate))
    });
    let (prefixes, inherited, inherited_encoded) = match (reusable, old_inherited) {
        (Some(prefixes), Some((inherited, encoded))) => (
            prefixes
                .iter()
                .map(|prefix| Prefix {
                    child: prefix.child.clone(),
                    base: prefix.base.clone(),
                    thread: prefix.thread.clone(),
                    candidate: prefix.candidate.clone(),
                    cut: prefix.cut,
                    digest: prefix.digest.clone(),
                })
                .collect::<Vec<_>>(),
            inherited,
            encoded,
        ),
        _ => {
            let mut chain = Chain {
                deps,
                cache: prefix_cache,
                segments: Vec::new(),
                raw_bytes: 0,
                prefixes: Vec::new(),
                seen: BTreeSet::from([uid_for(parsed.candidate.source, &parsed.candidate.path)]),
            };
            chain.inherit(
                parsed.candidate.source,
                &parsed.meta,
                &uid_for(parsed.candidate.source, &parsed.candidate.path),
            )?;
            let prefixes = std::mem::take(&mut chain.prefixes);
            let inherited = chain.inherited();
            let encoded = Arc::new(if files.retains() {
                encode_events(inherited.as_slice(), true, None)?
            } else {
                EncodedEvents::default()
            });
            (prefixes, inherited, encoded)
        }
    };
    let inherited_bytes = accounted_bytes(&inherited_encoded, inherited.iter());
    let digests = prefixes
        .iter()
        .map(|prefix| prefix.digest.clone())
        .collect::<Vec<_>>();
    let identity =
        history::inherited_identity(history::native_identity(&parsed, &request.agent), digests);
    let meta = view_meta(request, &parsed)?;
    let mut sources = vec![parsed.candidate.clone()];
    sources.extend(prefixes.iter().map(|prefix| prefix.candidate.clone()));
    let mut dependencies = BTreeSet::from([request.uid.clone()]);
    dependencies.insert(uid_for(parsed.candidate.source, &parsed.candidate.path));
    for prefix in &prefixes {
        dependencies.insert(uid_for(prefix.candidate.source, &prefix.candidate.path));
    }
    Ok(Built {
        view: View::new(ViewParts {
            parsed,
            meta,
            inherited,
            inherited_encoded,
            identity,
            native_scope,
            sources,
            dependencies: dependencies.into_iter().collect(),
        }),
        owner: owner_parsed,
        prefixes,
        inherited_encoded: inherited_bytes,
    })
}

/// Identity of the selected view: Claude sidecar `sessionId`
/// is compared with the owner's, Codex agents carry their own thread id.
fn native_scope(
    request: &ViewRequest,
    owner: &Parsed,
    selected: &Parsed,
) -> Result<NativeScope, SessionError> {
    let source = owner.candidate.source;
    if source != selected.candidate.source
        || !matches!(source, "claude" | "codex" | "grok" | "opencode" | "agy")
    {
        return Err(SessionError::new(501, "此数据源尚不支持原生操作范围"));
    }
    for parsed in [owner, selected] {
        if let Some(error) = parsed.raw_error.as_ref().or(parsed.unsupported.as_ref()) {
            return Err(SessionError::new(501, error.clone()));
        }
    }
    let owner_id = owner.native_id.as_ref().map_err(Clone::clone)?;
    let selected_id = selected.native_id.as_ref().map_err(Clone::clone)?;
    if source == "claude" && owner_id != selected_id {
        return Err(SessionError::new(
            409,
            "Claude 子代理声明的会话 ID 与主会话不一致",
        ));
    }
    Ok(NativeScope {
        source: source.to_owned(),
        uid: request.uid.clone(),
        session_id: if source == "claude" {
            owner_id
        } else {
            selected_id
        }
        .clone(),
        agent_id: (!request.agent.is_empty()).then(|| request.agent.clone()),
    })
}

/// Walks Codex `history_base` chains through the caller's index, reading each
/// parent's fixed prefix `[0, cut)` through its own checked handle. Prefixes
/// are reparsed independently: filtering a parent's full tail by offset would
/// retain later rollback/interrupt edits to earlier messages.
struct Chain<'a> {
    deps: &'a dyn Dependencies,
    cache: Option<&'a Mutex<PrefixCache>>,
    /// Projected prefixes, deepest ancestor first, already at physical end 0.
    segments: Vec<Arc<Vec<Event>>>,
    raw_bytes: usize,
    prefixes: Vec<Prefix>,
    seen: BTreeSet<String>,
}

impl Chain<'_> {
    /// The inherited events: one parent's prefix is shared as-is, a deeper
    /// chain is concatenated once.
    fn inherited(mut self) -> Arc<Vec<Event>> {
        match self.segments.len() {
            0 => Arc::new(Vec::new()),
            1 => self.segments.pop().expect("one segment"),
            _ => Arc::new(
                self.segments
                    .iter()
                    .flat_map(|segment| segment.iter().cloned())
                    .collect(),
            ),
        }
    }

    /// `[0, cut)` of `candidate` projected: from the shared cache when it holds
    /// exactly this stamp, else parsed now (and retained for the next fork).
    fn prefix(
        &self,
        candidate: &Candidate,
        sid: &str,
        cut: usize,
    ) -> Result<(Value, Arc<Vec<Event>>, String), SessionError> {
        let eviction = if let Some(cache) = self.cache {
            let mut cache = cache
                .lock()
                .map_err(|_| SessionError::new(500, "继承历史缓存锁不可用"))?;
            if let Some(hit) = cache.get(candidate, cut) {
                return Ok(hit);
            }
            Some(cache.eviction)
        } else {
            None
        };
        let (meta, events, digest) = parse_prefix(candidate, sid, cut)?;
        let events = Arc::new(
            events
                .into_iter()
                .map(|event| Event {
                    end: 0,
                    message: event.message,
                    media: event.media,
                })
                .collect::<Vec<_>>(),
        );
        if let Some(cache) = self.cache {
            // Byte accounting only: the prefix cache keeps events, not their bytes.
            let encoded = accounted_bytes(&encode_events(&events, false, None)?, events.iter());
            // Restamp outside the lock; an eviction during the read also
            // vetoes admission, even if the path was recreated with old bytes.
            let current = restamp(candidate)? == *candidate;
            let entry = PrefixEntry {
                candidate: candidate.clone(),
                meta: meta.clone(),
                events: events.clone(),
                digest: digest.clone(),
                encoded,
                used: Instant::now(),
            };
            let mut retired = Retired::default();
            if let Ok(mut cache) = cache.lock()
                && current
                && Some(cache.eviction) == eviction
            {
                cache.insert(cut, entry, &mut retired);
            }
        }
        Ok((meta, events, digest))
    }

    fn inherit(&mut self, source: &str, meta: &Value, child: &str) -> Result<(), SessionError> {
        let Some((sid, cut)) = history::history_link(source, meta)? else {
            return Ok(());
        };
        let candidate = self
            .deps
            .thread_from("codex", sid, child, &meta["history_base"])?;
        let uid = uid_for(candidate.source, &candidate.path);
        if !self.seen.insert(uid.clone()) {
            return Err(SessionError::new(501, "分叉历史依赖存在循环"));
        }
        let candidate = restamp(&candidate)?;
        self.raw_bytes = self
            .raw_bytes
            .checked_add(cut)
            .ok_or_else(|| SessionError::new(413, "继承历史预算溢出"))?;
        let (candidate, (prefix_meta, prefix_events, digest)) =
            match self.prefix(&candidate, sid, cut) {
                // A parent appended between the index's stat and this open is an
                // ordinary append of an unrelated tail: read it once more with
                // its current stamp before reporting a retry.
                Err(error) if error.status == 503 => {
                    let fresh = restamp(&candidate)?;
                    if fresh == candidate {
                        return Err(error);
                    }
                    let parsed = self.prefix(&fresh, sid, cut)?;
                    (fresh, parsed)
                }
                other => (candidate, other?),
            };
        // Zero is an empty prefix, not permission to include grandparents.
        if cut > 0 {
            self.inherit(candidate.source, &prefix_meta, &uid)?;
            self.segments.push(prefix_events);
        }
        self.prefixes.push(Prefix {
            child: child.to_owned(),
            base: meta["history_base"].clone(),
            thread: sid.to_owned(),
            digest: json!([uid, cut, digest]),
            candidate,
            cut,
        });
        Ok(())
    }
}

/// Read and project exactly `[0, cut)` of a parent candidate. The scan itself
/// proves the cut is a committed LF boundary and yields its prefix digest;
/// the parent's tail is never read, so its later edits cannot leak in.
fn parse_prefix(
    candidate: &Candidate,
    sid: &str,
    cut: usize,
) -> Result<(Value, Vec<Event>, String), SessionError> {
    let unsupported = |message: &str| SessionError::new(501, message.to_owned());
    let expected = candidate
        .data_stamp()
        .ok_or_else(|| unsupported("父历史缺少原生输入"))?;
    if cut as u64 > expected.size {
        return Err(unsupported("父历史固定前缀超出完整原生数据范围"));
    }
    if cut == 0 {
        return Ok((Value::Null, Vec::new(), format!("{:x}", Sha1::digest([]))));
    }
    let mut reader = native_input::CheckedNative::open_prefix(
        &candidate.root,
        &candidate.data,
        expected,
        cut as u64,
    )?;
    let mut decoder = records::Decoder::cold();
    let index = records::scan_native_records(&mut reader, &mut decoder, None, candidate)?;
    reader.finish()?;
    if index.committed() != cut as u64 {
        return Err(unsupported("父历史固定前缀不在完整 JSONL 行边界"));
    }
    let digest = index
        .prefix_hash(cut as u64)
        .expect("committed cut is a checkpoint");
    let batch = decoder.finish(cut);
    if let Some(error) = batch.error {
        return Err(unsupported(&format!("父历史前缀不受支持：{error}")));
    }
    let records = batch.records;
    let fallback = timestamp(expected.modified);
    let (meta, events, error) = providers::parse_with_media(
        candidate.source,
        &candidate.path,
        &records,
        None,
        &fallback,
        &batch.sidecars,
    );
    if let Some(error) = error {
        return Err(unsupported(&format!("父历史固定前缀不受支持：{error}")));
    }
    let parsed_sid = meta["sid"].as_str().unwrap_or("");
    let physical_id =
        super::index::codex_rollout_id(&candidate.data, parsed_sid).unwrap_or(parsed_sid);
    if physical_id != sid {
        return Err(unsupported("父历史固定前缀不包含相同线程身份"));
    }
    Ok((meta, events, digest))
}
