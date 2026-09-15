//! Bounded on-demand search over semantic session views, never raw JSONL.
//!
//! The pool is the frozen candidate list (published rows in `updated`
//! order). The searchable body of a candidate (`body`: the texts of the roles
//! searched, joined by newlines) comes from the persistent search-text
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
//! body whole.
//!
//! Before any body is opened, the query's prefilter (`prefilter`, over the
//! case-folded copies the cache keeps resident, `fold`) rules out the bodies
//! that cannot match. Literal and whole-word queries then run the regex
//! crate's literal search on the original text (`Matcher`), the whole-word
//! form checking each occurrence's neighbours against the boundary class
//! `[\p{L}\p{N}_]` instead of driving the backtracking engine across the
//! text; regex queries run `fancy-regex` as before.
//!
//! Wire compatibility: `q`, comma-separated `source`, `limit` (default 60,
//! optional), and `word/case/regex/progress` enabled by the value 1. Public session views
//! are the search pool; attached agent transcripts
//! are not silently merged into their owner's text. Unsupported views produce
//! `errors`, `partial` and `incomplete`, while readable results are retained.
//!
//! Search accepts lookaround and backreferences and follows the whole-word
//! boundary construction. The native session text is unchanged by matching.

pub mod cache;
pub mod fold;
pub mod prefilter;
pub mod service;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};

use fancy_regex::{Regex, RegexBuilder};
use regex::Regex as PlainRegex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::sessions::{SessionError, ViewSnapshot};
pub use cache::Cached;
pub use prefilter::Prefilter;

const HIT_CAP: usize = 200;
/// Characters of context around the first hit (40 before,
/// 150 after).
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
    /// The debug-run view the candidates come from.
    pub debug_run: String,
}

/// How one body is matched. Every variant finds the same spans as the
/// `fancy-regex` pattern the query used to compile to (`tests::reference`):
/// a literal is delegated to the regex crate by `fancy-regex` anyway, and the
/// whole-word form `(?<![\p{L}\p{N}_])(?:lit)(?![\p{L}\p{N}_])` matches
/// at `s` exactly when the literal matches at `s` and neither neighbour is
/// in the class — checked here per occurrence, so the engine never scans
/// the text between occurrences (its look-behind defeats the literal
/// prefilter, 3 s of CPU over 19 MB).
enum Matcher {
    Literal(PlainRegex),
    Word(PlainRegex),
    Regex(Regex),
}

impl Matcher {
    /// The leftmost match starting at or after `pos`, as the pattern's
    /// `find_from_pos` would report it.
    fn find_at(&self, hay: &str, pos: usize) -> Result<Option<(usize, usize)>, SearchError> {
        match self {
            Matcher::Literal(plain) => Ok(plain
                .find_at(hay, pos)
                .map(|found| (found.start(), found.end()))),
            Matcher::Word(plain) => {
                let mut pos = pos;
                while let Some(found) = plain.find_at(hay, pos) {
                    let (start, end) = (found.start(), found.end());
                    let before = hay[..start]
                        .chars()
                        .next_back()
                        .is_some_and(fold::is_word_char);
                    let after = hay[end..].chars().next().is_some_and(fold::is_word_char);
                    if !before && !after {
                        return Ok(Some((start, end)));
                    }
                    // Not a whole word here: the engine would try the next
                    // character, where only another occurrence can match.
                    pos = start + hay[start..].chars().next().map_or(1, char::len_utf8);
                    if pos > hay.len() {
                        break;
                    }
                }
                Ok(None)
            }
            Matcher::Regex(pattern) => pattern
                .find_from_pos(hay, pos)
                .map(|found| found.map(|found| (found.start(), found.end())))
                .map_err(|error| {
                    SearchError::new(
                        400,
                        "invalid_search_regex",
                        format!("正则匹配失败: {error}"),
                    )
                }),
        }
    }
}

pub struct PreparedSearch {
    matcher: Option<Matcher>,
    /// Bodies whose folded copy fails this cannot match and are skipped.
    prefilter: Prefilter,
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
        self.matcher.is_none()
    }

    pub fn chunkable(&self) -> bool {
        self.chunkable
    }

    pub fn prefilter(&self) -> &Prefilter {
        &self.prefilter
    }

    /// Whether a body with this folded copy can match.
    pub fn admits(&self, folded: &[u8]) -> bool {
        self.prefilter.admits(folded)
    }
}

fn invalid_regex(error: impl std::fmt::Display) -> SearchError {
    SearchError::new(400, "invalid_search_regex", format!("正则无效: {error}"))
}

/// The whole-word wrapper around a regex source.
fn whole_word(source: &str) -> String {
    format!(r"(?<![\p{{L}}\p{{N}}_])(?:{source})(?![\p{{L}}\p{{N}}_])")
}

/// The query's `fancy-regex` pattern: a regex query as written (with the
/// whole-word wrapper), a literal query escaped.
fn fancy_pattern(q: &str, word: bool, case: bool, regex: bool) -> Result<Regex, SearchError> {
    let source = if regex {
        q.to_owned()
    } else {
        regex::escape(q)
    };
    let source = if word { whole_word(&source) } else { source };
    RegexBuilder::new(&source)
        .case_insensitive(!case)
        .backtrack_limit(usize::MAX)
        .delegate_size_limit(usize::MAX)
        .delegate_dfa_size_limit(usize::MAX)
        .build()
        .map_err(invalid_regex)
}

pub fn empty_result() -> Value {
    json!({"results": [], "truncated": false, "total_pool": 0, "scanned": 0,
        "errors": [], "partial": false, "incomplete": false})
}

fn flag(value: &str) -> bool {
    value == "1"
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
        let word = flag(&self.word);
        let case = flag(&self.case);
        let regex = flag(&self.regex);
        let progress = flag(&self.progress);
        let limit = self.limit.parse::<usize>().unwrap_or(60);
        let sources: BTreeSet<_> = self
            .source
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        let chunkable = chunkable(&self.q, regex);
        let (matcher, prefilter) = if self.q.trim().is_empty() {
            (None, Prefilter::none())
        } else if regex {
            (
                Some(Matcher::Regex(fancy_pattern(&self.q, word, case, true)?)),
                Prefilter::regex(&self.q, !case),
            )
        } else {
            let plain = regex::RegexBuilder::new(&regex::escape(&self.q))
                .case_insensitive(!case)
                .size_limit(usize::MAX)
                .dfa_size_limit(usize::MAX)
                .build()
                .map_err(invalid_regex)?;
            (
                Some(if word {
                    Matcher::Word(plain)
                } else {
                    Matcher::Literal(plain)
                }),
                Prefilter::literal(&self.q),
            )
        };
        Ok(PreparedSearch {
            debug_run: self.debug_run.chars().take(64).collect(),
            matcher,
            prefilter,
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

fn ansi() -> &'static PlainRegex {
    static ANSI: OnceLock<PlainRegex> = OnceLock::new();
    ANSI.get_or_init(|| PlainRegex::new(r"\x1b\[[0-9;]*m").expect("constant regex"))
}

fn collapse(raw: &str) -> String {
    ansi()
        .replace_all(raw, "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The snippet: 40 characters before the first hit, 150 after, terminal
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
        let Some(matcher) = &query.matcher else {
            return Ok(());
        };
        let mut position = 0;
        while position <= chunk.len() {
            check_cancel(cancelled)?;
            let Some((start, end)) = matcher.find_at(chunk, position)? else {
                break;
            };
            if start == end && !final_chunk && start == chunk.len() {
                break;
            }
            self.count += 1;
            if self.snippet.is_none() && self.pending.is_none() {
                if final_chunk && self.tail.is_empty() {
                    self.snippet = Some(snippet(chunk, start, end));
                } else {
                    self.start_snippet(chunk, start, end);
                }
            }
            if self.count >= HIT_CAP {
                self.capped = true;
                break;
            }
            position = if end > start {
                end
            } else if let Some(next) = chunk[end..].chars().next() {
                end + next.len_utf8()
            } else {
                break;
            };
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

/// The searchable body of one view: the semantic texts of the roles
/// searched, joined by newlines. Media, cursors and private payloads never
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
/// cancellation); `emit` provides bounded backpressure and
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
    if query.matcher.is_none() {
        return Ok(empty_result());
    }
    let pool: Vec<_> = rows
        .iter()
        .filter(|row| {
            query.sources.is_empty() || query.sources.contains(row["source"].as_str().unwrap_or(""))
        })
        .collect();
    emit(json!({"type": "progress", "done": 0, "total": pool.len()}))?;
    let (mut hits, mut errors, mut scanned) = (Vec::new(), Vec::new(), 0);
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

    /// The query as it was matched before the literal and whole-word
    /// matchers: every shape compiled to one `fancy-regex` pattern and
    /// driven through the same scanner. The reference for equivalence.
    fn reference(q: &str, word: bool, case: bool, regex: bool) -> PreparedSearch {
        let mut query = SearchQuery {
            q: q.into(),
            word: if word { "1" } else { "0" }.into(),
            case: if case { "1" } else { "0" }.into(),
            regex: if regex { "1" } else { "0" }.into(),
            ..Default::default()
        }
        .prepare()
        .unwrap();
        if query.matcher.is_some() {
            query.matcher = Some(Matcher::Regex(fancy_pattern(q, word, case, regex).unwrap()));
        }
        query.prefilter = Prefilter::none();
        query
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
    fn python_regex_features_and_ordinary_flags_are_accepted() {
        assert_eq!(count(r"(?=cat)", "cat", false, true), 1);
        assert_eq!(count(r"(?<=cat)", "cat", false, true), 1);
        assert_eq!(count(r"(cat)\1", "catcat", false, true), 1);
        assert_eq!(count(r"(cat)\1", "catcat", true, true), 1);
        assert!(
            SearchQuery {
                q: "[".into(),
                regex: "1".into(),
                ..Default::default()
            }
            .prepare()
            .is_err()
        );
        assert!(
            SearchQuery {
                q: "test".into(),
                word: "true".into(),
                ..Default::default()
            }
            .prepare()
            .is_ok()
        );
        assert!(
            SearchQuery {
                q: "x".repeat(4097),
                ..Default::default()
            }
            .prepare()
            .is_ok()
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
        assert!(chunkable("sessiondock.*rust", true));
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

    /// Every query shape over a fixture of case-folding edge cases,
    /// overlapping occurrences, CJK, ANSI colours and chunk boundaries:
    /// the new matchers report exactly the hits, cap and snippet of the
    /// reference pattern, whole and chunked, and the prefilter admits every
    /// body that has a hit.
    #[test]
    fn matchers_equal_the_reference_pattern_and_prefilter_admits_every_hit() {
        let bodies = [
            "The ddp_guard hook\nDDP_GUARD again ddp_guardian\n_ddp_guard ddp_guard_",
            "a#a#a\naa#a\nxa#a#a\n#a#a#",
            "\u{212A}elvin \u{17F}ession kelvin session KELVIN SESSION",
            "İstanbul ıstanbul istanbul Istanbul İ ı i I",
            "Σίσυφος ΣΊΣΥΦΟΣ σίσυφος ς σ Σ",
            "straße STRASSE Straẞe ß ẞ",
            "猫 猫猫 _猫 猫\n会话列表 会話列表",
            "\x1b[31mguard\x1b[0m \x1b[1mGuard\x1b[0m Guard guard",
            "cat\u{0301} \u{203f}cat _cat cat9 9cat cat catcat CatCAT",
            "",
            "\n",
            "guard",
            "guard\n",
            "\nguard",
            "AgentHub was ported to Rust\nno match here at all",
        ];
        let long = format!(
            "{}\nguard {}\nGUARD{}\n{}",
            "x".repeat(300),
            "guard ".repeat(250),
            "y".repeat(400),
            "tail guard ddp_guard"
        );
        let queries: &[(&str, bool, bool, bool)] = &[
            ("ddp_guard", false, false, false),
            ("ddp_guard", false, true, false),
            ("guard", true, false, false),
            ("guard", true, true, false),
            ("Guard", false, false, false),
            ("Guard", true, true, false),
            ("a#a", false, false, false),
            ("a#a", true, false, false),
            ("#a", true, false, false),
            ("kelvin", false, false, false),
            ("\u{212A}elvin", false, false, false),
            ("session", true, false, false),
            ("ſession", true, false, false),
            ("i", false, false, false),
            ("İ", false, false, false),
            ("ı", true, false, false),
            ("σ", false, false, false),
            ("ς", true, false, false),
            ("ΣΊΣΥΦΟΣ", true, false, false),
            ("ß", false, false, false),
            ("straße", true, false, false),
            ("ẞ", false, true, false),
            ("猫", true, false, false),
            ("会话", false, false, false),
            ("cat", true, false, false),
            ("x", false, false, false),
            ("guard ", false, false, false),
            (" ", false, false, false),
            ("agenthub.*rust", false, false, true),
            ("guard|ddp_?guard", true, false, true),
            ("a|ab", true, false, true),
            ("()", true, false, true),
            ("n.*e", false, false, true),
            ("(?<!d)guard(?!i)", false, false, true),
            ("(cat)\\1", false, false, true),
            ("^guard$", false, false, true),
            ("\\bguard\\b", false, true, true),
            ("[σς]", true, false, true),
        ];
        let never = AtomicBool::new(false);
        for (q, word, case, regex) in queries {
            let new = SearchQuery {
                q: (*q).into(),
                word: if *word { "1" } else { "0" }.into(),
                case: if *case { "1" } else { "0" }.into(),
                regex: if *regex { "1" } else { "0" }.into(),
                ..Default::default()
            }
            .prepare()
            .unwrap();
            let old = reference(q, *word, *case, *regex);
            let mut hits = 0;
            for hay in bodies.iter().copied().chain([long.as_str()]) {
                let expected = matches(&old, hay, &never).unwrap();
                let got = matches(&new, hay, &never).unwrap();
                assert_eq!(
                    got, expected,
                    "{q:?} word={word} case={case} regex={regex} hay={hay:?}"
                );
                if expected.is_some() {
                    hits += 1;
                    assert!(
                        new.admits(&fold::fold(hay)),
                        "prefilter rejects a hit: {q:?} word={word} case={case} regex={regex} hay={hay:?}"
                    );
                }
                if new.chunkable() {
                    for lines in 1..=3 {
                        assert_eq!(
                            chunked(&new, hay, lines),
                            chunked(&old, hay, lines),
                            "{q:?} word={word} case={case} regex={regex} lines={lines} hay={hay:?}"
                        );
                    }
                }
            }
            assert!(hits > 0 || *q == " ", "{q:?} never matched the fixture");
        }
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
