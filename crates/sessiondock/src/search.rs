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
//! `[\p{L}\p{N}_]`, except a Han/kana/hangul/bopomofo letter beside a
//! non-CJK letter (`的tag` matches `tag`; `猫猫` does not match `猫`),
//! instead of driving the backtracking engine across the text; regex
//! queries run `fancy-regex` as before.
//!
//! Literal queries split on whitespace with double-quoted phrases; `mode=any`
//! selects OR, otherwise every term must occur somewhere in the session (AND).
//! Each term uses the literal/whole-word matcher, retaining discovery across
//! chunks, without a synthesized backtracking regex or a whole-body read.
//!
//! Wire compatibility: `q`, `mode`, comma-separated `source`, `limit` (default 60,
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
    /// `any` means OR; omitted or any other value means AND.
    pub mode: String,
    pub source: String,
    pub limit: String,
    pub word: String,
    pub case: String,
    pub regex: String,
    pub progress: String,
}

/// How one body is matched. Every variant finds the same spans as the
/// `fancy-regex` pattern the query used to compile to (`tests::reference`):
/// a literal is delegated to the regex crate by `fancy-regex` anyway, and the
/// whole-word form from `whole_word` matches at `s` exactly when the literal
/// matches at `s` and neither neighbour blocks (`fold::word_edge_blocks`):
/// a `[\p{L}\p{N}_]` neighbour blocks, unless it and the adjacent match
/// letter are on opposite sides of the CJK/non-CJK split. Checked here per
/// occurrence, so the engine never scans the text between occurrences (its
/// look-behind defeats the literal prefilter, 3 s of CPU over 19 MB).
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
                    let edge_first = hay[start..end].chars().next();
                    let edge_last = hay[start..end].chars().next_back();
                    let before = hay[..start]
                        .chars()
                        .next_back()
                        .is_some_and(|c| fold::word_edge_blocks(c, edge_first));
                    let after = hay[end..]
                        .chars()
                        .next()
                        .is_some_and(|c| fold::word_edge_blocks(c, edge_last));
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
    terms: Vec<PreparedSearch>,
    any: bool,
    /// Bodies whose folded copy fails this cannot match and are skipped.
    prefilter: Prefilter,
    /// The pattern cannot match across a newline, so a body can be matched
    /// as line-aligned chunks with exactly the whole-body outcome.
    chunkable: bool,
    sources: BTreeSet<String>,
    limit: usize,
    pub progress: bool,
}

impl PreparedSearch {
    pub fn is_empty(&self) -> bool {
        self.matcher.is_none() && self.terms.is_empty()
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
///
/// A neighbour in `[\p{L}\p{N}_]` blocks, except a CJK letter
/// (Han/Hiragana/Katakana/Hangul/Bopomofo) directly beside a non-CJK letter.
/// Digits and `_` still join, and a non-letter edge still blocks on any word
/// neighbour, matching `fold::word_edge_blocks`.
fn whole_word(source: &str) -> String {
    const CJK: &str =
        r"\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}\p{Script=Bopomofo}";
    let non_cjk_letter = format!(r"(?:[\p{{L}}&&[^{CJK}]])", CJK = CJK);
    let split = format!(
        r"(?<=[{CJK}])(?={non})|(?<={non})(?=[{CJK}])",
        CJK = CJK,
        non = non_cjk_letter,
    );
    // The leading check looks at the character before the match; the trailing
    // check looks at the character after it. The script-split alternatives
    // are the same text at those two positions.
    format!(r"(?:(?<![\p{{L}}\p{{N}}_])|{split})(?:{source})(?:(?![\p{{L}}\p{{N}}_])|{split})")
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

/// Whitespace separates literal terms; double quotes preserve a phrase.
/// An unfinished quote keeps the remainder as a phrase while typing.
/// Within quotes, \" and \\ can be escaped. Empty and duplicate terms disappear.
pub fn literal_terms(q: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut term = String::new();
    let mut quoted = false;
    let mut chars = q.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted && c == '\\' && matches!(chars.peek(), Some('"' | '\\')) {
            term.push(chars.next().unwrap());
        } else if c == '"' {
            quoted = !quoted;
        } else if !quoted && (c.is_whitespace() || c == '\u{feff}') {
            if !term.is_empty() {
                terms.push(std::mem::take(&mut term));
            }
        } else {
            term.push(c);
        }
    }
    if !term.is_empty() {
        terms.push(term);
    }
    let mut seen = BTreeSet::new();
    terms.retain(|term| seen.insert(term.clone()));
    terms
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
        let mut terms = if regex {
            Vec::new()
        } else {
            literal_terms(&self.q)
        };
        let any = self.mode == "any";
        if terms.len() > 1 {
            let prefilter = Prefilter::terms(&terms, any);
            let terms = terms
                .into_iter()
                .map(|q| {
                    // Quote the decoded term so spaces/newlines remain literal.
                    let q = format!("\"{}\"", q.replace('\\', "\\\\").replace('"', "\\\""));
                    SearchQuery {
                        q,
                        word: self.word.clone(),
                        case: self.case.clone(),
                        ..Default::default()
                    }
                    .prepare()
                })
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(PreparedSearch {
                chunkable: terms.iter().all(PreparedSearch::chunkable),
                matcher: None,
                terms,
                any,
                prefilter,
                sources,
                limit,
                progress,
            });
        }
        let q = if regex {
            self.q
        } else {
            terms.pop().unwrap_or_default()
        };
        let chunkable = chunkable(&q, regex);
        let (matcher, prefilter) = if q.is_empty() || (regex && q.trim().is_empty()) {
            (None, Prefilter::none())
        } else if regex {
            (
                Some(Matcher::Regex(fancy_pattern(&q, word, case, true)?)),
                Prefilter::regex(&q, !case),
            )
        } else {
            let plain = regex::RegexBuilder::new(&regex::escape(&q))
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
                Prefilter::literal(&q),
            )
        };
        Ok(PreparedSearch {
            matcher,
            terms: Vec::new(),
            any,
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
    terms: Vec<Scanner<'q>>,
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
            terms: query.terms.iter().map(Scanner::new).collect(),
            count: 0,
            capped: false,
            tail: String::new(),
            pending: None,
            snippet: None,
        }
    }

    /// Whether more chunks can still change the outcome.
    pub fn wants_more(&self) -> bool {
        if !self.terms.is_empty() {
            return self.terms.iter().map(|term| term.count).sum::<usize>() < HIT_CAP
                || (!self.query.any && self.terms.iter().any(|term| term.count == 0))
                || self.terms.iter().any(|term| term.pending.is_some());
        }
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
        if !self.terms.is_empty() {
            for term in &mut self.terms {
                if term.wants_more() {
                    term.feed(chunk, final_chunk, cancelled)?;
                }
            }
            return Ok(());
        }
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
        if !self.terms.is_empty() {
            let outcomes: Vec<_> = self.terms.into_iter().map(Scanner::finish).collect();
            if !self.query.any && outcomes.iter().any(Option::is_none) {
                return None;
            }
            let outcomes: Vec<_> = outcomes
                .into_iter()
                .enumerate()
                .filter_map(|(index, outcome)| outcome.map(|outcome| (index, outcome)))
                .collect();
            if outcomes.is_empty() {
                return None;
            }
            let count: usize = outcomes.iter().map(|(_, outcome)| outcome.0).sum();
            // Prefer the first-hit excerpt covering the most terms, then add
            // excerpts for terms occurring in other messages. No body retained.
            let never = AtomicBool::new(false);
            let mut excerpts: Vec<_> = outcomes
                .iter()
                .map(|(own_index, outcome)| {
                    let mut covered: BTreeSet<_> = self
                        .query
                        .terms
                        .iter()
                        .enumerate()
                        .filter_map(|(index, term)| {
                            matches(term, &outcome.2, &never)
                                .ok()
                                .flatten()
                                .map(|_| index)
                        })
                        .collect();
                    covered.insert(*own_index);
                    (outcome.2.clone(), covered)
                })
                .collect();
            let mut covered = BTreeSet::new();
            let mut snippets = Vec::new();
            while !excerpts.is_empty() {
                let index = excerpts
                    .iter()
                    .enumerate()
                    .max_by_key(|(index, (_, terms))| {
                        (
                            terms.difference(&covered).count(),
                            std::cmp::Reverse(*index),
                        )
                    })
                    .map(|(index, _)| index)
                    .unwrap();
                let (text, terms) = excerpts.remove(index);
                if snippets.is_empty() || terms.difference(&covered).next().is_some() {
                    covered.extend(terms);
                    snippets.push(text);
                }
            }
            return Some((count.min(HIT_CAP), count >= HIT_CAP, snippets.join(" … ")));
        }
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
    body_texts(view.texts())
}

/// The same body from a cold semantic projection, before detail encoding.
pub(crate) fn body_texts<'a>(texts: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    texts
        .filter(|(role, text)| SEARCH_ROLES.contains(role) && !text.is_empty())
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What one worker found for a candidate.
pub enum Scanned {
    Matched(Outcome),
    Session {
        main: Outcome,
        agents: Vec<Value>,
        errors: Vec<Value>,
    },
    Error(SessionError),
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
    if query.is_empty() {
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
                let (scanned_row, agents) = match scanned_row {
                    Scanned::Session {
                        main,
                        agents,
                        errors: agent_errors,
                    } => {
                        errors.extend(agent_errors);
                        // An owner with only sidecar hits supplies navigation
                        // context, with no claim that its own body matched.
                        let main = main.or_else(|| {
                            agents.first().map(|agent| {
                                (0, false, agent["snippet"].as_str().unwrap_or("").to_owned())
                            })
                        });
                        (Scanned::Matched(main), Some(agents))
                    }
                    other => (other, None),
                };
                match scanned_row {
                    Scanned::Matched(Some((count, capped, snippet))) => {
                        let mut hit = row.clone();
                        hit["agent_items"] = json!(agents.unwrap_or_default());
                        hit["hits"] = json!(count);
                        hit["hits_capped"] = json!(capped);
                        hit["snippet"] = json!(snippet);
                        emit(json!({"type": "matches", "results": [hit.clone()]}))?;
                        hits.push(hit);
                    }
                    Scanned::Matched(None) => {}
                    Scanned::Session { .. } => unreachable!("normalized above"),
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
