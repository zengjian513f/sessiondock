//! Bounded on-demand search over semantic session views, never raw JSONL.
//!
//! The pool is the frozen candidate list (published rows in `updated`
//! order). The searchable body of a candidate (`body`: the texts of the roles
//! Python searches, joined by newlines) comes from the persistent search-text
//! cache (`search::cache`, one private file per session and file version);
//! only a session whose file version is not cached is projected — a
//! still-current cached view is borrowed, anything else is streamed into a
//! transient view that is matched and dropped (docs/read-model.md "搜索").
//! Candidates are scanned by a bounded worker
//! pool; results are emitted strictly in pool order, so streaming packets,
//! `scanned`, `truncated` and the final ordering are exactly those of a
//! sequential scan.
//!
//! Cached bodies are matched as a stream of line-aligned chunks with a small
//! reusable buffer: no session body is read into memory whole unless the
//! query can match across a newline (`PreparedSearch::chunkable`), which only
//! a regex using escapes, classes, inline flags or anchors can; those read the
//! body whole under a bytes-in-flight budget.
//!
//! Wire compatibility: `q`, comma-separated `source`, `limit` (default 60,
//! 1..=200), and `word/case/regex/progress` encoded as 0/1. Public session views
//! are the search pool, matching Python's default; attached agent transcripts
//! are not silently merged into their owner's text. Unsupported views produce
//! `errors`, `partial` and `incomplete`, while readable results are retained.
//!
//! Regex compatibility: Rust regex 1 supports Unicode literals/classes,
//! alternation, repetition, anchors and inline flags. Lookaround and
//! backreferences are intentionally unsupported and return HTTP 400 before
//! either JSON or NDJSON starts. Unicode case folding follows regex 1, not
//! Python re's exact edge cases. Whole-word means Unicode letters/numbers/_
//! on both sides, including punctuation-led queries; it does not require a
//! regex lookbehind. Query size, compiled regex size and result bytes are
//! bounded. Cancellation is checked between views, chunks and matches, not
//! inside a single bounded regex engine call or snapshot parse.
//!
//! | Option/syntax | Compatibility contract |
//! | --- | --- |
//! | Literal mode | Escaped Unicode text, never interpreted as regex |
//! | `word=1` | Unicode letters/numbers/underscore boundary; no Rust `\b` shortcut |
//! | `case=0` | regex 1 Unicode simple folding; some Python `re.I` cases differ |
//! | Regex `\w`, `\b`, flags, anchors | regex 1 dialect; not a Python re emulation |
//! | Lookaround, backreferences, excessive pattern complexity | Explicit HTTP 400 |
//!
//! Engine reference: <https://docs.rs/regex/latest/regex/#syntax>.

pub mod cache;
pub mod service;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};

use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::sessions::{SessionError, ViewSnapshot};
pub use cache::Cached;

const QUERY_BYTES: usize = 4096;
const RESULT_BYTES: usize = 8 * 1024 * 1024;
const HIT_CAP: usize = 200;
/// Characters of context around the first hit (Python `m.start() - 40`,
/// `m.end() + 150`).
const SNIPPET_BEFORE: usize = 40;
const SNIPPET_AFTER: usize = 150;
const SEARCH_ROLES: &[&str] = &[
    "user",
    "assistant",
    "user·subagent",
    "assistant·subagent",
    "thinking",
    "question",
    "answer",
];

#[derive(Debug)]
pub struct SearchError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl SearchError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn cancelled() -> Self {
        Self::new(499, "search_cancelled", "搜索已取消")
    }
}

impl From<SessionError> for SearchError {
    fn from(error: SessionError) -> Self {
        Self::new(
            error.status,
            if error.status == 501 {
                "unsupported_history"
            } else {
                "session_error"
            },
            error.message,
        )
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct SearchQuery {
    pub q: String,
    pub source: String,
    pub limit: String,
    pub word: String,
    pub case: String,
    pub regex: String,
    pub progress: String,
    /// Python `_debug_run`: the debug-run view the candidates come from.
    pub debug_run: String,
}

pub struct PreparedSearch {
    pattern: Option<Regex>,
    continued_word_pattern: Option<Regex>,
    word: bool,
    /// The pattern cannot match across a newline, so a body can be matched
    /// as line-aligned chunks with exactly the whole-body outcome.
    chunkable: bool,
    sources: BTreeSet<String>,
    limit: usize,
    pub progress: bool,
    pub debug_run: String,
}

impl PreparedSearch {
    pub fn is_empty(&self) -> bool {
        self.pattern.is_none()
    }

    pub fn chunkable(&self) -> bool {
        self.chunkable
    }
}

pub fn empty_result() -> Value {
    json!({"results": [], "truncated": false, "total_pool": 0, "scanned": 0,
        "errors": [], "partial": false, "incomplete": false})
}

fn flag(value: &str) -> Result<bool, SearchError> {
    match value {
        "" | "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(SearchError::new(
            400,
            "invalid_search_query",
            "搜索开关仅接受 0 或 1",
        )),
    }
}

/// Whether a pattern provably never matches a newline: a literal query
/// without one, or a regex made only of literals, `.` (which excludes `\n`
/// without the `s` flag), grouping, alternation and repetition. Escapes
/// (`\s`, `\n`, `\W`, `\p{..}`), classes (`[^x]`), inline flags (`(?s)`) and
/// anchors (`^`/`$` would re-anchor at every chunk) fall back to whole-body
/// matching; conservative on purpose.
fn chunkable(q: &str, regex: bool) -> bool {
    if q.contains(['\n', '\r']) {
        return false;
    }
    if !regex {
        return true;
    }
    !q.contains(['\\', '[', '^', '$']) && !q.contains("(?")
}

impl SearchQuery {
    pub fn prepare(self) -> Result<PreparedSearch, SearchError> {
        if self.q.len() > QUERY_BYTES {
            return Err(SearchError::new(
                400,
                "search_query_too_long",
                "搜索词超过 4096 字节限制",
            ));
        }
        let word = flag(&self.word)?;
        let case = flag(&self.case)?;
        let regex = flag(&self.regex)?;
        let progress = flag(&self.progress)?;
        let limit = if self.limit.is_empty() {
            60
        } else {
            self.limit.parse::<usize>().map_err(|_| {
                SearchError::new(400, "invalid_search_limit", "limit 必须为 1 至 200 的整数")
            })?
        };
        if !(1..=200).contains(&limit) {
            return Err(SearchError::new(
                400,
                "invalid_search_limit",
                "limit 必须为 1 至 200 的整数",
            ));
        }
        let sources: BTreeSet<_> = self
            .source
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if sources
            .iter()
            .any(|s| !["claude", "codex", "grok"].contains(&s.as_str()))
        {
            return Err(SearchError::new(
                400,
                "invalid_search_source",
                "未知搜索来源；仅支持 claude、codex、grok",
            ));
        }
        let chunkable = chunkable(&self.q, regex);
        let (pattern, continued_word_pattern) = if self.q.trim().is_empty() {
            (None, None)
        } else {
            let source = if regex {
                self.q
            } else {
                regex::escape(&self.q)
            };
            // Capture only the user's match, while consuming boundary separators.
            // Reuse consumed separators in the scanner for adjacent matches;
            // this also preserves regex alternative backtracking (a|ab on ab).
            let build = |source: &str| {
                RegexBuilder::new(source).case_insensitive(!case)
                .size_limit(1024 * 1024).dfa_size_limit(2 * 1024 * 1024).nest_limit(64)
                .build().map_err(|error| SearchError::new(400, "invalid_search_regex",
                    format!("正则无效或超出 Rust 搜索语法/复杂度限制；不支持前后查找（lookaround）和反向引用（backreference）：{error}")))
            };
            if word {
                (
                    Some(build(&format!(
                        r"(?:^|[^\p{{L}}\p{{N}}_])({source})(?:$|[^\p{{L}}\p{{N}}_])"
                    ))?),
                    Some(build(&format!(
                        r"[^\p{{L}}\p{{N}}_]({source})(?:$|[^\p{{L}}\p{{N}}_])"
                    ))?),
                )
            } else {
                (Some(build(&source)?), None)
            }
        };
        Ok(PreparedSearch {
            debug_run: self.debug_run.chars().take(64).collect(),
            pattern,
            continued_word_pattern,
            word,
            chunkable,
            sources,
            limit,
            progress,
        })
    }
}

fn check_cancel(cancelled: &AtomicBool) -> Result<(), SearchError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(SearchError::cancelled())
    } else {
        Ok(())
    }
}

fn ansi() -> &'static Regex {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;]*m").expect("constant regex"))
}

fn collapse(raw: &str) -> String {
    ansi()
        .replace_all(raw, "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Python's snippet: 40 characters before the first hit, 150 after, terminal
/// colours removed and whitespace collapsed.
fn snippet(haystack: &str, start: usize, end: usize) -> String {
    let lo = haystack[..start]
        .char_indices()
        .rev()
        .nth(SNIPPET_BEFORE - 1)
        .map_or(0, |(offset, _)| offset);
    let hi = haystack[end..]
        .char_indices()
        .nth(SNIPPET_AFTER)
        .map_or(haystack.len(), |(offset, _)| end + offset);
    collapse(&haystack[lo..hi])
}

/// The first hit's context while it may still extend into the next chunk.
struct PendingSnippet {
    raw: String,
    /// Characters still wanted after the hit.
    need: usize,
}

/// The outcome of matching one body: `(hits, capped, snippet)`.
pub type Outcome = Option<(usize, bool, String)>;

/// Matches one body fed as chunks. With a chunkable query every chunk but
/// the last ends with `\n` (`cache::TextReader::for_each_chunk`), so a hit
/// never straddles chunks and the result equals one whole-body scan; a
/// non-chunkable query must be fed the body as a single final chunk.
pub struct Scanner<'q> {
    query: &'q PreparedSearch,
    count: usize,
    capped: bool,
    /// Up to 40 characters ending the previous chunk, for the first hit's context.
    tail: String,
    pending: Option<PendingSnippet>,
    snippet: Option<String>,
}

impl<'q> Scanner<'q> {
    pub fn new(query: &'q PreparedSearch) -> Self {
        Self {
            query,
            count: 0,
            capped: false,
            tail: String::new(),
            pending: None,
            snippet: None,
        }
    }

    /// Whether more chunks can still change the outcome.
    pub fn wants_more(&self) -> bool {
        !self.capped || self.pending.is_some()
    }

    fn remember_tail(&mut self, chunk: &str) {
        let keep = chunk
            .char_indices()
            .rev()
            .nth(SNIPPET_BEFORE - 1)
            .map_or(0, |(offset, _)| offset);
        if keep == 0 {
            self.tail.push_str(chunk);
            let trim = self
                .tail
                .char_indices()
                .rev()
                .nth(SNIPPET_BEFORE - 1)
                .map_or(0, |(offset, _)| offset);
            self.tail.drain(..trim);
        } else {
            self.tail.clear();
            self.tail.push_str(&chunk[keep..]);
        }
    }

    fn start_snippet(&mut self, chunk: &str, start: usize, end: usize) {
        let mut raw = String::new();
        let mut before = chunk[..start].char_indices().rev();
        match before.nth(SNIPPET_BEFORE - 1) {
            Some((lo, _)) => raw.push_str(&chunk[lo..start]),
            None => {
                // Fewer than 40 characters in this chunk: the rest of the
                // context ends the previous chunk.
                let have = chunk[..start].chars().count();
                let lo = self
                    .tail
                    .char_indices()
                    .rev()
                    .nth(SNIPPET_BEFORE - have - 1)
                    .map_or(0, |(offset, _)| offset);
                raw.push_str(&self.tail[lo..]);
                raw.push_str(&chunk[..start]);
            }
        }
        raw.push_str(&chunk[start..end]);
        let after = &chunk[end..];
        match after.char_indices().nth(SNIPPET_AFTER) {
            Some((offset, _)) => {
                raw.push_str(&after[..offset]);
                self.snippet = Some(collapse(&raw));
            }
            None => {
                let got = after.chars().count();
                raw.push_str(after);
                self.pending = Some(PendingSnippet {
                    raw,
                    need: SNIPPET_AFTER - got,
                });
            }
        }
    }

    fn continue_snippet(&mut self, chunk: &str, final_chunk: bool) {
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        match chunk.char_indices().nth(pending.need) {
            Some((offset, _)) => {
                pending.raw.push_str(&chunk[..offset]);
                self.snippet = Some(collapse(&pending.raw));
            }
            None => {
                pending.need -= chunk.chars().count();
                pending.raw.push_str(chunk);
                if final_chunk {
                    self.snippet = Some(collapse(&pending.raw));
                } else {
                    self.pending = Some(pending);
                }
            }
        }
    }

    /// Match `chunk`; `final_chunk` marks the end of the body. An empty
    /// match at the end of a non-final chunk is the same position as the
    /// next chunk's start and is counted there.
    pub fn feed(
        &mut self,
        chunk: &str,
        final_chunk: bool,
        cancelled: &AtomicBool,
    ) -> Result<(), SearchError> {
        if self.pending.is_some() {
            self.continue_snippet(chunk, final_chunk);
        }
        if self.capped {
            return Ok(());
        }
        let query = self.query;
        let Some(pattern) = &query.pattern else {
            return Ok(());
        };
        let (mut cursor, mut first_in_chunk) = (0, true);
        loop {
            check_cancel(cancelled)?;
            let found = if query.word {
                let pattern = if first_in_chunk {
                    pattern
                } else {
                    query
                        .continued_word_pattern
                        .as_ref()
                        .expect("word continuation pattern")
                };
                pattern
                    .captures_at(chunk, cursor)
                    .and_then(|groups| groups.get(1))
            } else {
                pattern.find_at(chunk, cursor)
            };
            let Some(found) = found else { break };
            if found.is_empty() && !final_chunk && found.start() == chunk.len() {
                break;
            }
            first_in_chunk = false;
            self.count += 1;
            if self.snippet.is_none() && self.pending.is_none() {
                if final_chunk && self.tail.is_empty() {
                    self.snippet = Some(snippet(chunk, found.start(), found.end()));
                } else {
                    self.start_snippet(chunk, found.start(), found.end());
                }
            }
            if self.count >= HIT_CAP {
                self.capped = true;
                break;
            }
            cursor = found.end();
            if query.word {
                if found.is_empty() {
                    if cursor == chunk.len() {
                        break;
                    }
                } else {
                    // Reuse the preceding separator for adjacent punctuation
                    // matches ("#" on "##"). Removing the ^ alternative prevents
                    // replaying a match at zero; empty matches advance via their
                    // mandatory consumed prefix instead of skipping a character.
                    cursor = chunk[..cursor]
                        .char_indices()
                        .next_back()
                        .map_or(0, |(offset, _)| offset);
                }
            } else if found.is_empty() {
                let Some(next) = chunk[cursor..].chars().next() else {
                    break;
                };
                cursor += next.len_utf8();
            }
        }
        if !final_chunk {
            self.remember_tail(chunk);
        }
        Ok(())
    }

    pub fn finish(mut self) -> Outcome {
        if let Some(pending) = self.pending.take() {
            self.snippet = Some(collapse(&pending.raw));
        }
        (self.count > 0).then(|| (self.count, self.capped, self.snippet.unwrap_or_default()))
    }
}

/// Match one whole body.
pub fn matches(
    query: &PreparedSearch,
    haystack: &str,
    cancelled: &AtomicBool,
) -> Result<Outcome, SearchError> {
    let mut scanner = Scanner::new(query);
    scanner.feed(haystack, true, cancelled)?;
    Ok(scanner.finish())
}

/// The searchable body of one view: the semantic texts of the roles Python
/// searches, joined by newlines. Media, cursors and private payloads never
/// enter it.
pub fn body(view: &ViewSnapshot) -> String {
    view.texts()
        .filter(|(role, text)| SEARCH_ROLES.contains(role) && !text.is_empty())
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What one worker found for a candidate.
pub enum Scanned {
    Matched(Outcome),
    Error(SessionError),
}

/// Match the body of an opened view (or carry its open error): the direct
/// path without the search-text cache, for callers that hold views.
pub fn scan_view(
    view: Result<std::sync::Arc<ViewSnapshot>, SessionError>,
    query: &PreparedSearch,
    cancelled: &AtomicBool,
) -> Scanned {
    match view {
        Ok(view) => {
            let text = body(&view);
            drop(view);
            match matches(query, &text, cancelled) {
                Ok(outcome) => Scanned::Matched(outcome),
                Err(error) => Scanned::Error(SessionError {
                    status: error.status,
                    message: error.message,
                }),
            }
        }
        Err(error) => Scanned::Error(error),
    }
}

/// Runs on an admitted blocking worker. `rows` is the frozen candidate list
/// in published order; `scan` matches one candidate's body (cached, borrowed
/// or projected, see `service`) with the worker's reusable chunk buffer, and
/// its error becomes that row's `errors` entry. Up to `workers` candidates
/// are scanned at once, but `emit` sees hits and progress strictly in pool
/// order and the scan stops where a sequential one would (`limit`,
/// cancellation, result budget); `emit` provides bounded backpressure and
/// returning an error stops work immediately. No partial history is guessed.
pub fn execute(
    rows: &[Value],
    scan: impl Fn(&str, &AtomicBool, &mut Vec<u8>) -> Scanned + Sync,
    query: &PreparedSearch,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(Value) -> Result<(), SearchError>,
    workers: usize,
) -> Result<Value, SearchError> {
    check_cancel(cancelled)?;
    if query.pattern.is_none() {
        return Ok(empty_result());
    }
    let pool: Vec<_> = rows
        .iter()
        .filter(|row| {
            query.sources.is_empty() || query.sources.contains(row["source"].as_str().unwrap_or(""))
        })
        .collect();
    emit(json!({"type": "progress", "done": 0, "total": pool.len()}))?;
    let (mut hits, mut errors, mut scanned, mut bytes) = (Vec::new(), Vec::new(), 0, 0);
    let mut truncated = false;
    let stop = AtomicBool::new(false);
    let next = AtomicUsize::new(0);
    let workers = workers.clamp(1, pool.len().max(1));
    let outcome = std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel::<(usize, Scanned)>();
        for _ in 0..workers {
            let sender = sender.clone();
            let (pool, next, stop, scan) = (&pool, &next, &stop, &scan);
            scope.spawn(move || {
                let mut buffer = Vec::new();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= pool.len() || stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }
                    let uid = pool[index]["uid"].as_str().unwrap_or("");
                    let result = scan(uid, cancelled, &mut buffer);
                    if sender.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut buffered = BTreeMap::new();
        let mut wanted = 0;
        let result = (|| {
            while wanted < pool.len() {
                check_cancel(cancelled)?;
                if hits.len() >= query.limit {
                    truncated = true;
                    break;
                }
                let scanned_row = match buffered.remove(&wanted) {
                    Some(found) => found,
                    None => match receiver.recv() {
                        Ok((index, found)) if index == wanted => found,
                        Ok((index, found)) => {
                            buffered.insert(index, found);
                            continue;
                        }
                        Err(_) => return Err(SearchError::cancelled()),
                    },
                };
                let row = pool[wanted];
                let uid = row["uid"].as_str().unwrap_or("");
                match scanned_row {
                    Scanned::Matched(Some((count, capped, snippet))) => {
                        let mut hit = row.clone();
                        hit["hits"] = json!(count);
                        hit["hits_capped"] = json!(capped);
                        hit["snippet"] = json!(snippet);
                        let size = serde_json::to_vec(&hit)
                            .expect("JSON value serializes")
                            .len();
                        if size > RESULT_BYTES - bytes {
                            errors.push(json!({"uid": uid, "source": row["source"], "name": row["title"],
                                "status": 413, "code": "search_result_limit", "error": "搜索结果超过 8 MiB 预算，请细化条件"}));
                            truncated = true;
                            scanned += 1;
                            emit(
                                json!({"type": "progress", "done": scanned, "total": pool.len()}),
                            )?;
                            break;
                        }
                        bytes += size;
                        emit(json!({"type": "matches", "results": [hit.clone()]}))?;
                        hits.push(hit);
                    }
                    Scanned::Matched(None) => {}
                    Scanned::Error(error) => {
                        if error.status == 499 {
                            return Err(SearchError::cancelled());
                        }
                        errors.push(json!({"uid": uid, "source": row["source"], "name": row["title"],
                            "status": error.status, "code": if error.status == 501 { "unsupported_history" } else { "session_error" },
                            "error": error.message}));
                    }
                }
                scanned += 1;
                wanted += 1;
                emit(json!({"type": "progress", "done": scanned, "total": pool.len()}))?;
            }
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        drop(receiver);
        result
    });
    outcome?;
    hits.sort_by(|left, right| {
        right["updated"]
            .as_str()
            .cmp(&left["updated"].as_str())
            .then_with(|| left["uid"].as_str().cmp(&right["uid"].as_str()))
    });
    let partial = !errors.is_empty();
    Ok(
        json!({"results": hits, "truncated": truncated, "total_pool": pool.len(), "scanned": scanned,
        "errors": errors, "partial": partial, "incomplete": partial || truncated}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(q: &str, word: bool, regex: bool) -> PreparedSearch {
        SearchQuery {
            q: q.into(),
            word: if word { "1" } else { "0" }.into(),
            regex: if regex { "1" } else { "0" }.into(),
            ..Default::default()
        }
        .prepare()
        .unwrap()
    }

    fn count(q: &str, hay: &str, word: bool, regex: bool) -> usize {
        matches(&prepared(q, word, regex), hay, &AtomicBool::new(false))
            .unwrap()
            .map_or(0, |value| value.0)
    }

    /// The whole-body outcome and the chunked outcome must agree for a
    /// chunkable query, for every line-aligned split of the body.
    fn chunked(query: &PreparedSearch, hay: &str, chunk_lines: usize) -> Outcome {
        let mut scanner = Scanner::new(query);
        let lines: Vec<&str> = hay.split_inclusive('\n').collect();
        let mut chunks: Vec<String> = lines
            .chunks(chunk_lines.max(1))
            .map(|lines| lines.concat())
            .collect();
        if chunks.is_empty() {
            chunks.push(String::new());
        }
        let never = AtomicBool::new(false);
        for (index, chunk) in chunks.iter().enumerate() {
            scanner
                .feed(chunk, index + 1 == chunks.len(), &never)
                .unwrap();
        }
        scanner.finish()
    }

    #[test]
    fn whole_word_alternation_neighbors_unicode_and_punctuation() {
        assert_eq!(count("a|ab", "ab ab ax", true, true), 2);
        assert_eq!(count("猫", "猫 猫猫 _猫 猫", true, false), 2);
        assert_eq!(count("#tag", "#tag x#tag #tagx #tag", true, false), 2);
        assert_eq!(count("#", "##", true, false), 2);
        assert_eq!(count("#|()", "##", true, true), 3);
        assert_eq!(count("cat", "Cat caterpillar CAT cat", true, false), 3);
        assert_eq!(count("cat", "cat\u{0301} \u{203f}cat _cat", true, false), 2);
    }

    #[test]
    fn bounded_zero_width_and_hit_cap() {
        assert_eq!(count("^|$", "猫", false, true), 2);
        assert_eq!(count("()", "...", true, true), 4);
        assert_eq!(count("a", &"a".repeat(400), false, false), 200);
    }

    #[test]
    fn unsupported_patterns_and_invalid_flags_are_client_errors() {
        for expression in [r"(?=cat)", r"(?<=cat)", r"(cat)\1", "["] {
            assert_eq!(
                SearchQuery {
                    q: expression.into(),
                    regex: "1".into(),
                    ..Default::default()
                }
                .prepare()
                .err()
                .unwrap()
                .status,
                400
            );
        }
        assert!(
            SearchQuery {
                q: "test".into(),
                word: "true".into(),
                ..Default::default()
            }
            .prepare()
            .is_err()
        );
    }

    #[test]
    fn snippets_are_unicode_safe_and_strip_terminal_colors() {
        assert_eq!(
            snippet("猫 \x1b[31mneedle\x1b[0m tail", 8, 14),
            "猫 needle tail"
        );
    }

    #[test]
    fn chunkable_queries_are_the_newline_free_ones() {
        assert!(chunkable("ddp_guard", false));
        assert!(
            chunkable("a\\sb", false),
            "literal mode escapes the backslash"
        );
        assert!(!chunkable("a\nb", false));
        assert!(chunkable("agenthub.*rust", true));
        assert!(chunkable("guard|ddp_?guard", true));
        for risky in [r"a\sb", "[^x]", "(?s)a.b", "^foo", "foo$", r"\n"] {
            assert!(!chunkable(risky, true), "{risky}");
        }
        assert!(prepared("x", false, false).chunkable());
        assert!(!prepared(r"x\s", false, true).chunkable());
    }

    #[test]
    fn chunked_scan_equals_whole_body_scan() {
        let body = "第一行 needle here\nsecond needle\n\nNEEDLE at start\n#needle# ##\nlast needle";
        let long = format!(
            "{}\n{}\nneedle{}\n{}",
            "x".repeat(300),
            "needle ".repeat(250),
            "y".repeat(400),
            "tail needle"
        );
        let cases = [
            ("needle", false, false),
            ("needle", true, false),
            ("#", true, false),
            ("needle|need", true, true),
            ("a|ab", true, true),
            ("()", true, true),
            ("n.*e", false, true),
            ("zzz", false, false),
            ("x", false, false),
        ];
        let never = AtomicBool::new(false);
        for hay in [
            body,
            long.as_str(),
            "",
            "\n",
            "needle",
            "needle\n",
            "\nneedle",
        ] {
            for (q, word, regex) in cases {
                let query = prepared(q, word, regex);
                let whole = matches(&query, hay, &never).unwrap();
                for lines in 1..=4 {
                    assert_eq!(
                        chunked(&query, hay, lines),
                        whole,
                        "{q:?} word={word} regex={regex} lines={lines} hay={hay:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn snippet_context_crosses_chunks() {
        let before = "b".repeat(60);
        let after = "a".repeat(200);
        let hay = format!("{before}\nX needle Y\n{after}\nend");
        let query = prepared("needle", false, false);
        let whole = matches(&query, &hay, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(
            whole.2.len(),
            40 + 6 + 150,
            "40 chars before (newline collapsed to a space), 150 after"
        );
        assert_eq!(chunked(&query, &hay, 1).unwrap(), whole);
        assert_eq!(chunked(&query, &hay, 2).unwrap(), whole);
    }

    #[test]
    fn execute_emits_in_pool_order_and_stops_at_limit() {
        let rows: Vec<Value> = (0..10)
            .map(|i| {
                json!({"uid": format!("claude:{i:016x}"), "source": "claude",
                "title": format!("t{i}"), "updated": format!("2026-01-{:02}T00:00:00Z", 10 - i)})
            })
            .collect();
        let bodies: BTreeMap<String, &str> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                (
                    row["uid"].as_str().unwrap().to_owned(),
                    if i % 3 == 0 {
                        "needle found"
                    } else {
                        "nothing"
                    },
                )
            })
            .collect();
        let scan = |uid: &str, cancelled: &AtomicBool, _buffer: &mut Vec<u8>| {
            if uid.ends_with('5') {
                return Scanned::Error(SessionError {
                    status: 501,
                    message: "坏".into(),
                });
            }
            let query = prepared("needle", false, false);
            Scanned::Matched(matches(&query, bodies[uid], cancelled).unwrap())
        };
        let mut packets = Vec::new();
        let query = SearchQuery {
            q: "needle".into(),
            limit: "2".into(),
            ..Default::default()
        }
        .prepare()
        .unwrap();
        let result = execute(
            &rows,
            scan,
            &query,
            &AtomicBool::new(false),
            |packet| {
                packets.push(packet);
                Ok(())
            },
            4,
        )
        .unwrap();
        let hits: Vec<&str> = result["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["uid"].as_str().unwrap())
            .collect();
        assert_eq!(hits, ["claude:0000000000000000", "claude:0000000000000003"]);
        assert_eq!(result["truncated"], true);
        assert_eq!(
            result["scanned"], 4,
            "sequential stop: rows 0..4 scanned, the limit stops before row 4"
        );
        assert_eq!(
            result["errors"].as_array().unwrap().len(),
            0,
            "row 5 is never reached"
        );
        let progress: Vec<u64> = packets
            .iter()
            .filter(|p| p["type"] == "progress")
            .map(|p| p["done"].as_u64().unwrap())
            .collect();
        assert_eq!(progress, (0..=4).collect::<Vec<_>>());
        // Without a limit the error row is reported and every row scanned.
        let query = SearchQuery {
            q: "needle".into(),
            ..Default::default()
        }
        .prepare()
        .unwrap();
        let result = execute(&rows, scan, &query, &AtomicBool::new(false), |_| Ok(()), 3).unwrap();
        assert_eq!(result["scanned"], 10);
        assert_eq!(result["results"].as_array().unwrap().len(), 4);
        assert_eq!(result["errors"][0]["uid"], "claude:0000000000000005");
        assert_eq!(result["partial"], true);
        let matched: Vec<&str> = packets
            .iter()
            .filter(|p| p["type"] == "matches")
            .map(|p| p["results"][0]["uid"].as_str().unwrap())
            .collect();
        assert_eq!(matched, hits);
    }
}
