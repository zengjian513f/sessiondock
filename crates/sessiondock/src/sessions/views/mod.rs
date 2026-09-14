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
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use super::{
    CURSOR_SCHEMA, Candidate, MessageQuery, NativeFence, NativeInputRead, NativeScope,
    NativeUserInput, PageStore, RewindTarget, SessionError, budgets, claude_agent_of, hash,
    history, media_projection, native_input, pages, providers, records, restamp, scope, timestamp,
    uid_for,
};
use crate::metadata::TimelinePin;

/// LRU bounds of the view cache (docs/read-model.md: 视图缓存 64 项 / 2 GiB).
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
    #[cfg(test)]
    pub _fixture: Option<Arc<tempfile::TempDir>>,
    pub committed: usize,
    pub meta: Value,
    /// Errors apply only to native-scope consumers, not compatible history display.
    pub native_id: Result<String, SessionError>,
    pub events: Vec<Event>,
    pub unsupported: Option<String>,
    pub raw_error: Option<String>,
    pub semantic_digest: String,
    /// The persisted Claude display pin these events were projected with
    /// (main sessions only). A different pin means a different logical view.
    pub pin: Option<TimelinePin>,
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
    /// Serialized message bytes plus resident media, the unit of the view
    /// budgets. Serialization stays on the bounded blocking worker.
    fn encoded_bytes(&self) -> Result<usize, SessionError> {
        encoded_bytes(self.events.iter())
    }
}

fn encoded_bytes<'a>(events: impl Iterator<Item = &'a Event>) -> Result<usize, SessionError> {
    let mut size = 0usize;
    for event in events {
        size = size.saturating_add(
            serde_json::to_vec(&event.message)
                .map_err(|_| SessionError::new(500, "消息序列化失败"))?
                .len(),
        );
        size = size.saturating_add(
            event
                .media
                .iter()
                .map(crate::media::NativeImage::resident_len)
                .fold(0usize, usize::saturating_add),
        );
    }
    Ok(size)
}

/// The resolved logical view behind a [`ViewSnapshot`]: one parsed leaf plus
/// the fixed inherited prefix events (physical end zero) that precede it.
pub(crate) struct View {
    pub parsed: Arc<Parsed>,
    pub meta: Value,
    pub inherited: Arc<Vec<Event>>,
    pub identity: String,
    pub native_scope: Result<NativeScope, SessionError>,
    /// Every candidate file this view was projected from, leaf first, then
    /// inherited parents. Native span reads authorize against these stamps.
    pub sources: Vec<Candidate>,
    /// Owner UIDs whose entries this view depends on (leaf, owner, parents).
    pub dependencies: Vec<String>,
    /// Python `CodexAdapter.read`: a Codex main session renamed through the
    /// name index shows the local `/rename <name>` as an inferred, uncounted
    /// `command` event at `renamed_at` (before the first later message).
    /// Derived from `meta` alone, so a rename recomposes without a reparse;
    /// its physical end is 0 — a full read carries it, an append never does.
    pub rename: Option<Event>,
}

/// Python `_name_event` + the `read` insertion: the `/rename` command event
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
/// first event whose `ts` is later than the rename (Python's `msgs.insert`).
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
    #[cfg(test)]
    pub fn all_events(&self) -> Vec<Event> {
        self.events().cloned().collect()
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

    /// HTTP-only media projection. Select the validated branch/window first;
    /// search and other text consumers never decode/register image payloads.
    /// Run on the same bounded blocking reader as ordinary message encoding.
    pub fn messages_with_media(
        &self,
        query: &MessageQuery,
        media: &crate::media::MediaStore,
    ) -> Result<Value, SessionError> {
        validate_message_query(query)?;
        message_batch(self, query, Some(media), None, None)
    }

    #[cfg(test)]
    pub(crate) fn messages_with_files(
        &self,
        query: &MessageQuery,
        media: &crate::media::MediaStore,
        files: Option<&crate::files::FileService>,
    ) -> Result<Value, SessionError> {
        validate_message_query(query)?;
        message_batch(self, query, Some(media), files, None)
    }

    pub(crate) fn messages_with_pages(
        &self,
        query: &MessageQuery,
        media: &crate::media::MediaStore,
        files: Option<&crate::files::FileService>,
        pages: &PageStore,
    ) -> Result<Value, SessionError> {
        validate_message_query(query)?;
        message_batch(self, query, Some(media), files, Some(pages))
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
    /// inferred rename event is not searchable (Python `search_only`).
    pub fn texts(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.view.events().filter_map(|event| {
            let role = event.message["role"].as_str()?;
            if role == "status" || event.message["inferred"] == true {
                return None;
            }
            Some((role, event.message["text"].as_str().unwrap_or("")))
        })
    }

    /// Delivery executor read: the current fence of a Claude main
    /// session plus the projected human `user` inputs committed after `from`,
    /// taken from this checked, restamped immutable view (no ad-hoc file
    /// access). The fence is validated exactly like a message checkpoint; an
    /// invalid fence yields no inputs rather than a guess.
    pub fn claude_native_inputs(
        &self,
        from: Option<&NativeFence>,
    ) -> Result<NativeInputRead, SessionError> {
        let view = &self.view;
        let scope = view.native_scope.clone()?;
        if scope.source != "claude" || scope.agent_id.is_some() {
            return Err(SessionError::new(400, "可靠发送只支持 Claude 主会话"));
        }
        let parsed = &view.parsed;
        let current = NativeFence {
            source_identity: view.identity.clone(),
            offset: parsed.committed as u64,
            head: self.head.clone(),
            anchor: self.anchor.clone(),
        };
        let Some(from) = from else {
            return Ok(NativeInputRead {
                scope,
                current,
                fence_valid: true,
                inputs: Vec::new(),
            });
        };
        let fence_valid = from.source_identity == view.identity
            && self.valid_checkpoint(&MessageQuery {
                start: from.offset,
                head: from.head.clone(),
                anchor: from.anchor.clone(),
                ..Default::default()
            });
        let mut inputs = Vec::new();
        if fence_valid {
            for event in view.events() {
                let message = &event.message;
                if event.end <= from.offset
                    || message["role"] != "user"
                    || message["interrupted"] == true
                {
                    continue;
                }
                let Some(uuid) = message["turn_id"].as_str().filter(|id| !id.is_empty()) else {
                    continue;
                };
                inputs.push(NativeUserInput {
                    uuid: uuid.to_owned(),
                    start: parsed.raw_index.record_start(event.end),
                    end: event.end,
                    text: message["text"].as_str().unwrap_or("").to_owned(),
                    ts: message["ts"].as_str().unwrap_or("").to_owned(),
                });
            }
        }
        Ok(NativeInputRead {
            scope,
            current,
            fence_valid,
            inputs,
        })
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

#[cfg(test)]
pub(crate) fn read_bounded(
    root: &std::path::Path,
    path: &std::path::Path,
    expected: &super::FileStamp,
) -> Result<Vec<u8>, SessionError> {
    read_bounded_limit(root, path, expected)
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
    // Python Grok falls back to chat mtime, or summary mtime if chat is absent;
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
    let (mut meta, events, provider_error) = providers::parse_options_with_media(
        candidate.source,
        &candidate.path,
        records,
        summary.as_ref(),
        &fallback,
        providers::ParseOptions {
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
        },
        &record_batch.sidecars,
    );
    if let (Some(pin), Some(outcome)) = (&pin, &outcome) {
        meta["timeline_pin"] = json!({
            "target": pin.target, "tip": pin.tip, "stale_end": pin.stale_end,
            "pinned_at": pin.pinned_at, "retired": outcome.retired.is_some(),
            "native_rewind": false,
        });
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
        // Python `_dir_size`: the whole session directory, like the row.
        meta["size"] = json!(
            super::index::directory_size(&candidate.root, &candidate.path)
                .unwrap_or_else(|| candidate.stamps.iter().map(|stamp| stamp.size).sum::<u64>())
        );
        meta["chat_exists"] = json!(candidate.data_stamp().is_some());
    }
    meta["supported"] = json!(unsupported.is_none());
    // A hard failure is the only warning of an unsupported row; a supported row
    // keeps the provider's non-fatal notes (unknown kinds skipped like Python).
    meta["migration_warnings"] = match &unsupported {
        Some(reason) => json!([reason]),
        None => meta["migration_warnings"]
            .as_array()
            .cloned()
            .map(Value::Array)
            .unwrap_or_else(|| json!([])),
    };
    let semantic_digest = projection_digest(&events, committed);
    let (native_id, _declared) = if candidate.source == "grok" {
        scope::grok_native_identity(summary.as_ref())
    } else {
        scope::native_identity(candidate.source, records)
    };
    cache.retain(candidate.clone(), record_batch);
    let mut parsed = Parsed {
        native_id,
        candidate,
        raw_index,
        #[cfg(test)]
        _fixture: None,
        committed,
        meta,
        events,
        unsupported,
        raw_error,
        semantic_digest,
        pin,
    };
    parsed.meta["cursor"] = json!({
        "end": committed, "head": parsed.head(committed),
        "anchor": semantic_anchor(&history::native_identity(&parsed, &agent), &parsed, committed),
    });
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
        parsed.semantic_digest.clone()
    } else {
        projection_digest(&parsed.events, end)
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

fn message_batch(
    snapshot: &ViewSnapshot,
    query: &MessageQuery,
    media: Option<&crate::media::MediaStore>,
    files: Option<&crate::files::FileService>,
    pages: Option<&PageStore>,
) -> Result<Value, SessionError> {
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
    let partial = if reset
        && query.window == "1"
        && let Some(pages) = pages
    {
        pages::window(snapshot, &mut selected, pages)?
    } else if reset && query.window == "1" && selected.len() > 600 {
        let omitted = selected.len() - 600;
        selected.drain(100..selected.len() - 500);
        json!({"head": 100, "tail": 500, "omitted": omitted})
    } else {
        Value::Null
    };
    let messages = project_selected(snapshot, &selected, media, files, pages)?;
    let current_anchor = &snapshot.anchor;
    let mut meta = view.meta.clone();
    meta["cursor"] = json!({"end": parsed.committed,
        "head": parsed.head(parsed.committed), "anchor": current_anchor});
    let response = json!({
        "meta": meta,
        "version": {"size": parsed.raw_index.length(),
                    "exists": parsed.candidate.data_stamp().is_some(),
                    "mtime": parsed.candidate.data_stamp().map(|stamp| stamp.modified / 1_000_000),
                    "head": parsed.head(parsed.committed)},
        "reset": reset, "start": if append_reset { parsed.committed } else { begin },
        "end": parsed.committed, "anchor": current_anchor,
        "messages": messages, "message_total": total, "partial": partial,
        "activity_changed": activity_changed, "activity": activity,
    });
    if pages.is_some() && reset && query.window == "1" {
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
    /// applied by the index); Python's `messages.meta` is this row, so the
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
    /// A parse of exactly this candidate (same stamps) and pin the caller
    /// already holds, so one file is never streamed twice for one change.
    /// Optional; the lazy index has none.
    fn parsed(&self, _candidate: &Candidate, _pin: Option<&TimelinePin>) -> Option<Arc<Parsed>> {
        None
    }
}

struct FileEntry {
    parsed: Arc<Parsed>,
    encoded: usize,
    used: Instant,
}

/// One inherited fixed prefix, with the stamp of the parent file it was read
/// from: an unchanged stamp lets a rebuild of the child skip re-reading it.
struct Prefix {
    /// The thread id the child declared for this parent.
    thread: String,
    candidate: Candidate,
    cut: usize,
    digest: Value,
}

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
            let resolved = deps.thread(prefix.candidate.source, &prefix.thread)?;
            if resolved.path != prefix.candidate.path
                || restamp(&prefix.candidate)? != prefix.candidate
            {
                return Ok(false);
            }
        }
        Ok(true)
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
    let rename = rename_event(&meta);
    Ok(Arc::new(ViewSnapshot::new(Arc::new(View {
        parsed: old.parsed.clone(),
        meta,
        inherited: old.inherited.clone(),
        identity: old.identity.clone(),
        native_scope: old.native_scope.clone(),
        sources: old.sources.clone(),
        dependencies: old.dependencies.clone(),
        rename,
    }))))
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ViewStats {
    /// Composed (uid, agent) views currently cached.
    pub views: usize,
    /// Parsed candidate files currently retained (leafs, owners).
    pub files: usize,
    /// Serialized message bytes (plus resident media) retained by them.
    pub bytes: usize,
}

/// Bounded LRU of opened sessions. Never consulted by the list.
#[derive(Default)]
pub(crate) struct Views {
    files: BTreeMap<String, FileEntry>,
    views: BTreeMap<(String, String), CachedView>,
    records: records::RecordCache,
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
}

impl Views {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Open (or refresh) the view for `request`. Cheap when nothing changed:
    /// one `stat` per involved file. Run on the bounded blocking reader.
    pub(crate) fn open(
        &mut self,
        request: &ViewRequest,
        deps: &dyn Dependencies,
    ) -> Result<Arc<ViewSnapshot>, SessionError> {
        validate_request(request)?;
        let key = (request.uid.clone(), request.agent.clone());
        let owner = restamp(&request.owner)?;
        let selected = request.selected.as_ref().map(restamp).transpose()?;
        let leaf = selected.clone().unwrap_or_else(|| owner.clone());
        let pin = leaf_pin(request, &leaf);
        if let Some(cached) = self.views.get(&key)
            && cached.is_current(&leaf, &owner, pin.as_ref(), deps)?
        {
            let now = Instant::now();
            let snapshot = recompose(request, &cached.snapshot)?;
            let cached = self.views.get_mut(&key).expect("checked above");
            cached.snapshot = snapshot.clone();
            cached.used = now;
            for id in snapshot.view.dependencies.clone() {
                if let Some(entry) = self.files.get_mut(&id) {
                    entry.used = now;
                }
            }
            return Ok(snapshot);
        }
        let previous = self.views.remove(&key);
        let built = build(
            request,
            deps,
            &mut CachedFiles { views: self },
            owner,
            selected,
            pin.clone(),
            previous.as_ref().map(|cached| cached.prefixes.as_slice()),
            previous
                .as_ref()
                .map(|cached| cached.snapshot.view.inherited.clone()),
        )?;
        let snapshot = Arc::new(ViewSnapshot::new(Arc::new(built.view)));
        self.views.insert(
            key,
            CachedView {
                snapshot: snapshot.clone(),
                owner: built.owner,
                prefixes: built.prefixes,
                pin,
                inherited_encoded: built.inherited_encoded,
                used: Instant::now(),
            },
        );
        self.prune();
        Ok(snapshot)
    }

    /// A view for one bounded scan (search): the cached view when it is
    /// still current, otherwise a cold projection that is neither retained
    /// nor allowed to charge the shared AST cache. `Views` is borrowed only
    /// for the cheap cache probe; callers stream the miss outside any lock.
    pub(crate) fn cached_current(
        &mut self,
        request: &ViewRequest,
        deps: &dyn Dependencies,
    ) -> Result<Option<Arc<ViewSnapshot>>, SessionError> {
        validate_request(request)?;
        let key = (request.uid.clone(), request.agent.clone());
        let Some(cached) = self.views.get_mut(&key) else {
            return Ok(None);
        };
        let owner = restamp(&request.owner)?;
        let leaf = match &request.selected {
            Some(selected) => restamp(selected)?,
            None => owner.clone(),
        };
        if !cached.is_current(&leaf, &owner, leaf_pin(request, &leaf).as_ref(), deps)? {
            return Ok(None);
        }
        cached.used = Instant::now();
        // The caller's row (a frozen search pool) wins over the cached
        // meta; the cache itself is not rewritten by a scan.
        Ok(Some(recompose(request, &cached.snapshot)?))
    }

    /// Ensure the leaf of `request` is parsed (through the cache) and resolve
    /// a Claude rewind target against it.
    pub(crate) fn claude_rewind_target(
        &mut self,
        request: &ViewRequest,
        deps: &dyn Dependencies,
        target: &str,
    ) -> Result<RewindTarget, SessionError> {
        let snapshot = self.open(request, deps)?;
        claude_rewind_target(&snapshot.view.parsed, target)
    }

    /// Drop every view of this owner UID and the parsed files behind them.
    pub(crate) fn evict(&mut self, uid: &str) {
        self.views.retain(|key, _| key.0 != uid);
        self.files.remove(uid);
        self.prune();
    }

    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.views.clear();
        self.files.clear();
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> ViewStats {
        ViewStats {
            views: self.views.len(),
            files: self.files.len(),
            bytes: self.bytes(),
        }
    }

    #[cfg(test)]
    pub(crate) fn records(&self) -> &records::RecordCache {
        &self.records
    }
    /// The cached view of `(uid, agent)` as last opened, without any file
    /// check: the facade compares its stamps with the index before it lets
    /// the list borrow the view's anchor or pin state.
    pub(crate) fn cached(&self, uid: &str, agent: &str) -> Option<&Arc<ViewSnapshot>> {
        self.views
            .get(&(uid.to_owned(), agent.to_owned()))
            .map(|cached| &cached.snapshot)
    }

    fn bytes(&self) -> usize {
        self.files
            .values()
            .map(|entry| entry.encoded)
            .chain(self.views.values().map(|cached| cached.inherited_encoded))
            .fold(0, usize::saturating_add)
    }

    /// A parsed file for `candidate`, reusing the exact retained parse or the
    /// caller's, else extending/rebuilding from the last parse of that file.
    fn file(
        &mut self,
        candidate: Candidate,
        pin: Pin<'_>,
        deps: &dyn Dependencies,
    ) -> Result<(Arc<Parsed>, usize), SessionError> {
        let id = uid_for(candidate.source, &candidate.path);
        let now = Instant::now();
        if let Some(entry) = self.files.get_mut(&id)
            && entry.parsed.candidate == candidate
            && match pin {
                Pin::Exact(pin) => entry.parsed.pin.as_ref() == pin,
                Pin::Any => true,
            }
        {
            entry.used = now;
            return Ok((entry.parsed.clone(), entry.encoded));
        }
        let pin = match pin {
            Pin::Exact(pin) => pin,
            Pin::Any => None,
        };
        let mut fresh = false;
        let parsed = match deps.parsed(&candidate, pin) {
            Some(parsed) => parsed,
            None => {
                let previous = self.files.get(&id).map(|entry| entry.parsed.clone());
                fresh = true;
                Arc::new(parse_candidate(
                    candidate,
                    previous.as_deref(),
                    &mut self.records,
                    pin.cloned(),
                )?)
            }
        };
        let encoded = parsed.encoded_bytes()?;
        // The cap is serialized event bytes, not a promise about RSS.
        self.files.remove(&id);
        let (view_limit, view_byte_limit) = (view_limit(), view_byte_limit());
        let mut evicted = fresh;
        while self.files.len() >= view_limit
            || self.bytes().saturating_add(encoded) > view_byte_limit
        {
            let oldest = self
                .files
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            self.files.remove(&oldest);
            evicted = true;
        }
        self.files.insert(
            id,
            FileEntry {
                parsed: parsed.clone(),
                encoded,
                used: now,
            },
        );
        if evicted {
            // A fresh parse leaves its line buffers, the previous AST and an
            // over-budget decoded tree free but still mapped; an evicted parse
            // may still be referenced by a snapshot in flight. The trim only
            // returns what is already free.
            crate::sessions::memory::release();
        }
        Ok((parsed, encoded))
    }

    /// Views whose files were evicted or replaced are stale; drop them, then
    /// keep the view count within its bound (files are bounded on insert).
    fn prune(&mut self) {
        let files = &self.files;
        self.views.retain(|_, cached| {
            let leaf = &cached.snapshot.view.parsed;
            let current = |parsed: &Arc<Parsed>| {
                files
                    .get(&uid_for(parsed.candidate.source, &parsed.candidate.path))
                    .is_some_and(|entry| Arc::ptr_eq(&entry.parsed, parsed))
            };
            current(leaf) && cached.owner.as_ref().is_none_or(current)
        });
        while self.views.len() > view_limit() {
            let oldest = self
                .views
                .iter()
                .min_by_key(|(_, cached)| cached.used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            self.views.remove(&oldest);
        }
    }
}

struct CachedFiles<'a> {
    views: &'a mut Views,
}
impl FileSource for CachedFiles<'_> {
    fn file(
        &mut self,
        candidate: Candidate,
        pin: Pin<'_>,
        deps: &dyn Dependencies,
    ) -> Result<(Arc<Parsed>, usize), SessionError> {
        self.views.file(candidate, pin, deps)
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
            None => Arc::new(parse_candidate(
                candidate,
                None,
                &mut self.records,
                pin.cloned(),
            )?),
        };
        // Nothing will extend this parse: free the decoded records now, not
        // when the whole transient open ends (search cold paths run several
        // of these at once; the AST is the bulk of a projection's memory).
        self.records = records::RecordCache::default();
        let encoded = parsed.encoded_bytes()?;
        Ok((parsed, encoded))
    }
}

/// Project one session without any cache: the result is returned, not
/// retained, and the ASTs it decoded are dropped with this call.
pub(crate) fn open_transient(
    request: &ViewRequest,
    deps: &dyn Dependencies,
) -> Result<Arc<ViewSnapshot>, SessionError> {
    validate_request(request)?;
    let owner = restamp(&request.owner)?;
    let selected = request.selected.as_ref().map(restamp).transpose()?;
    let leaf = selected.clone().unwrap_or_else(|| owner.clone());
    let pin = leaf_pin(request, &leaf);
    let mut files = ColdFiles {
        records: records::RecordCache::default(),
    };
    let built = build(request, deps, &mut files, owner, selected, pin, None, None)?;
    Ok(Arc::new(ViewSnapshot::new(Arc::new(built.view))))
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

/// View metadata from the published owner row: Python's `session_view`
/// (`index.py`) for an agent, the row itself for the main transcript. The
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
    owner: Candidate,
    selected: Option<Candidate>,
    pin: Option<TimelinePin>,
    old_prefixes: Option<&[Prefix]>,
    old_inherited: Option<Arc<Vec<Event>>>,
) -> Result<Built, SessionError> {
    let leaf = selected.clone().unwrap_or_else(|| owner.clone());
    let (parsed, _) = files.file(leaf, Pin::Exact(pin.as_ref()), deps)?;
    let owner_parsed = match selected {
        Some(_) => Some(files.file(owner, Pin::Any, deps)?.0),
        None => None,
    };
    if let Some(error) = parsed.raw_error.as_ref().or(parsed.unsupported.as_ref()) {
        return Err(SessionError::new(501, error.clone()));
    }
    let native_scope = native_scope(request, owner_parsed.as_deref().unwrap_or(&parsed), &parsed);
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
    let (prefixes, inherited) = match (reusable, old_inherited) {
        (Some(prefixes), Some(inherited)) => (
            prefixes
                .iter()
                .map(|prefix| Prefix {
                    thread: prefix.thread.clone(),
                    candidate: prefix.candidate.clone(),
                    cut: prefix.cut,
                    digest: prefix.digest.clone(),
                })
                .collect::<Vec<_>>(),
            inherited,
        ),
        _ => {
            let mut chain = Chain {
                deps,
                events: Vec::new(),
                raw_bytes: 0,
                prefixes: Vec::new(),
                seen: BTreeSet::from([uid_for(parsed.candidate.source, &parsed.candidate.path)]),
            };
            chain.inherit(parsed.candidate.source, &parsed.meta)?;
            (chain.prefixes, Arc::new(chain.events))
        }
    };
    let inherited_encoded = encoded_bytes(inherited.iter())?;
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
    let rename = rename_event(&meta);
    Ok(Built {
        view: View {
            parsed,
            meta,
            inherited,
            identity,
            native_scope,
            sources,
            dependencies: dependencies.into_iter().collect(),
            rename,
        },
        owner: owner_parsed,
        prefixes,
        inherited_encoded,
    })
}

/// Identity of the selected view: Python compares Claude sidecar
/// `sessionId` with the owner's, Codex agents carry their own thread id.
fn native_scope(
    request: &ViewRequest,
    owner: &Parsed,
    selected: &Parsed,
) -> Result<NativeScope, SessionError> {
    let source = owner.candidate.source;
    if source != selected.candidate.source || !matches!(source, "claude" | "codex" | "grok") {
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
    events: Vec<Event>,
    raw_bytes: usize,
    prefixes: Vec<Prefix>,
    seen: BTreeSet<String>,
}

impl Chain<'_> {
    fn inherit(&mut self, source: &str, meta: &Value) -> Result<(), SessionError> {
        let Some((sid, cut)) = history::history_link(source, meta)? else {
            return Ok(());
        };
        let candidate = self.deps.thread("codex", sid)?;
        let uid = uid_for(candidate.source, &candidate.path);
        if !self.seen.insert(uid.clone()) {
            return Err(SessionError::new(501, "分叉历史依赖存在循环"));
        }
        let candidate = restamp(&candidate)?;
        self.raw_bytes = self
            .raw_bytes
            .checked_add(cut)
            .ok_or_else(|| SessionError::new(413, "继承历史预算溢出"))?;
        let (prefix_meta, prefix_events, digest) = match parse_prefix(&candidate, sid, cut) {
            // A parent appended between the index's stat and this open is an
            // ordinary append of an unrelated tail: read it once more with
            // its current stamp before reporting a retry.
            Err(error) if error.status == 503 => {
                let fresh = restamp(&candidate)?;
                if fresh == candidate {
                    return Err(error);
                }
                parse_prefix(&fresh, sid, cut)?
            }
            other => other?,
        };
        // Zero is an empty prefix, not permission to include grandparents.
        if cut > 0 {
            self.inherit(candidate.source, &prefix_meta)?;
            self.events
                .extend(prefix_events.into_iter().map(|event| Event {
                    end: 0,
                    message: event.message,
                    media: event.media,
                }));
        }
        self.prefixes.push(Prefix {
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
    if meta["sid"].as_str().unwrap_or("") != sid {
        return Err(unsupported("父历史固定前缀不包含相同线程身份"));
    }
    Ok((meta, events, digest))
}

#[cfg(test)]
mod tests;
