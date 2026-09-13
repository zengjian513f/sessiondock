//! Private, bounded JSON structure scanner. This is not a media classifier or
//! a file capability: every span is initially unauthorized ordinary text.
//!
//! Input must be limited by the caller to ONE complete JSON record. Success
//! requires EOF after JSON whitespace; partial JSONL tails are the caller's
//! responsibility. Offsets are relative to that input, not a native file.
//! The fixed 8 KiB input/hash buffers never grow with a large string. At most
//! `physical_bytes + 1` bytes are read (the extra byte distinguishes exact EOF
//! from oversized input). Resident accounting is a conservative logical AST
//! weight, including string capacities; it is not an allocator/RSS measurement.
use indexmap::IndexMap;
use serde_json::{Number, Value};
use sha1::{Digest, Sha1};
use std::io::Read;

const BUFFER: usize = 8192;
const HARD_DEPTH: usize = 128;
// Covers spare vector/ordered-map slots (including key headers and cached
// hashes), independently of node/key count limits. KEY_WEIGHT pays hash-table
// bookkeeping; owned key/text buffers additionally charge actual capacity.
const NODE_WEIGHT: usize = 2
    * (std::mem::size_of::<Node>() + std::mem::size_of::<String>() + std::mem::size_of::<usize>())
    + 64;
const KEY_WEIGHT: usize = 96;

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub physical_bytes: u64,
    pub inline_string_bytes: usize,
    pub resident_bytes: usize,
    pub depth: usize,
    pub nodes: usize,
    pub keys: usize,
    pub key_bytes: usize,
    pub number_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            physical_bytes: 256 * 1024 * 1024,
            inline_string_bytes: 64 * 1024,
            resident_bytes: 2 * 1024 * 1024,
            depth: 64,
            nodes: 100_000,
            keys: 50_000,
            key_bytes: 16 * 1024,
            number_bytes: 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Syntax,
    Utf8,
    DuplicateKey,
    Io,
    PhysicalLimit,
    ResidentLimit,
    StringLimit,
    DepthLimit,
    NodeLimit,
    KeyLimit,
    NumberLimit,
    UnmaterializedSpan,
    InvalidLimits,
}
/// Deliberately contains no input snippets, key names, paths or reader errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScanError {
    pub kind: ErrorKind,
    pub offset: u64,
}
impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "native JSON scan {:?} at byte {}",
            self.kind, self.offset
        )
    }
}
impl std::error::Error for ScanError {}

// Strict resident production parsing rejects spans; their source metadata is
// retained for the future, separately authorized extraction/streaming boundary.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct TextSpan {
    start: u64,
    end: u64,
    decoded_len: u64,
    digest: [u8; 20],
    escaped: bool,
    prefix: Vec<u8>,
    data_suffix: Option<(u64, [u8; 20])>,
}
#[cfg_attr(not(test), allow(dead_code))]
impl TextSpan {
    /// Half-open PHYSICAL range inside (excluding) the JSON double quotes.
    pub(crate) fn start(&self) -> u64 {
        self.start
    }
    pub(crate) fn end(&self) -> u64 {
        self.end
    }
    /// UTF-8 byte count AFTER JSON unescaping, not character/base64 byte count.
    pub(crate) fn decoded_len(&self) -> u64 {
        self.decoded_len
    }
    /// SHA-1 over ALL decoded UTF-8 bytes, including the discarded inline prefix.
    /// This content fingerprint is not by itself an authorization or MAC.
    pub(crate) fn digest(&self) -> &[u8; 20] {
        &self.digest
    }
    pub(crate) fn escaped(&self) -> bool {
        self.escaped
    }
    /// Bounded decoded bytes, potentially ending inside a Unicode scalar.
    pub(crate) fn prefix(&self) -> &[u8] {
        &self.prefix
    }
    /// Pure lexical evidence after the first comma of a `data:` candidate.
    /// Neither a MIME check nor media authorization.
    pub(crate) fn data_suffix(&self) -> Option<(u64, [u8; 20])> {
        self.data_suffix
    }
}
pub(crate) enum Text {
    Inline(String),
    Span(TextSpan),
}
impl Text {
    /// Ordinary text cannot silently become empty/a marker when not resident.
    /// Future authorized streaming readers must validate their own span source.
    pub(crate) fn into_string(self) -> Result<String, ScanError> {
        match self {
            Self::Inline(value) => Ok(value),
            Self::Span(span) => Err(ScanError {
                kind: ErrorKind::UnmaterializedSpan,
                offset: span.start,
            }),
        }
    }
}
// The private span-capable tree remains available for future extraction; normal
// production resident decoding now chooses the direct Value constructor.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum Node {
    Null,
    Bool(bool),
    Number(Number),
    String(Text),
    Array(Vec<Node>),
    // preserve_order is part of the existing serde_json/provider contract:
    // tool summaries consume object insertion order, not merely Value equality.
    Object(IndexMap<String, Node>),
}
#[cfg_attr(not(test), allow(dead_code))]
impl Node {
    pub(crate) fn has_span(&self) -> bool {
        match self {
            Self::String(Text::Span(_)) => true,
            Self::Array(values) => values.iter().any(Self::has_span),
            Self::Object(values) => values.values().any(Self::has_span),
            _ => false,
        }
    }
    /// Replace every remaining private span with text the caller reads back
    /// from its own checked source (verified against the span's decoded
    /// length/SHA-1). Ordinary giant text only: image spans were already
    /// discharged by the provider classifier before this runs, and nothing
    /// here infers media or opens a path.
    pub(crate) fn materialize_spans(
        &mut self,
        materialize: &mut impl FnMut(&TextSpan) -> Result<String, String>,
    ) -> Result<(), String> {
        match self {
            Self::String(text @ Text::Span(_)) => {
                let Text::Span(span) = &*text else {
                    unreachable!("matched a span")
                };
                *text = Text::Inline(materialize(span)?);
                Ok(())
            }
            Self::Array(values) => values
                .iter_mut()
                .try_for_each(|value| value.materialize_spans(materialize)),
            Self::Object(values) => values
                .values_mut()
                .try_for_each(|value| value.materialize_spans(materialize)),
            _ => Ok(()),
        }
    }
    /// Move small retained data into Value. No giant readback, placeholders,
    /// deep clones, image inference or media authorization occurs here.
    pub(crate) fn into_value(self) -> Result<Value, ScanError> {
        Ok(match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(value),
            Self::Number(value) => Value::Number(value),
            Self::String(value) => Value::String(value.into_string()?),
            Self::Array(values) => Value::Array(
                values
                    .into_iter()
                    .map(Self::into_value)
                    .collect::<Result<_, _>>()?,
            ),
            Self::Object(values) => Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| Ok((key, value.into_value()?)))
                    .collect::<Result<_, ScanError>>()?,
            ),
        })
    }
}
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScanStats {
    pub physical_bytes: u64,
    pub nodes: usize,
    pub keys: usize,
    pub resident_bytes: usize,
    pub peak_resident_bytes: usize,
    pub span_count: usize,
}
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Document {
    pub root: Node,
    // Accounting evidence for scanner tests and future streamed record indexes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub stats: ScanStats,
}
#[cfg_attr(not(test), allow(dead_code))]
impl Document {
    pub(crate) fn into_value(self) -> Result<Value, ScanError> {
        self.root.into_value()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn scan<R: Read>(reader: R, limits: Limits) -> Result<Document, ScanError> {
    let (root, stats) = scan_into::<_, Node>(reader, limits)?;
    Ok(Document { root, stats })
}

/// Strict resident decode using the same grammar, ordering, duplicate-key and
/// budget checks as `scan`, but building Value containers directly. A private
/// span is rejected explicitly, never replaced with a marker/empty value.
pub(crate) fn scan_value<R: Read>(reader: R, limits: Limits) -> Result<Value, ScanError> {
    scan_into::<_, Value>(reader, limits).map(|(value, _)| value)
}

// Output construction is the ONLY variable part of parsing. Logical charges
// deliberately remain based on the conservative Node shape even when Value's
// representation is lighter; the fast path does not relax input admission.
trait Build: Sized {
    type Map: Default;
    fn null() -> Self;
    fn boolean(value: bool) -> Self;
    fn number(value: Number) -> Self;
    fn text(value: Text) -> Result<Self, ScanError>;
    fn array(values: Vec<Self>) -> Self;
    fn object(values: Self::Map) -> Self;
    fn contains(values: &Self::Map, key: &str) -> bool;
    fn insert(values: &mut Self::Map, key: String, value: Self);
}
impl Build for Node {
    type Map = IndexMap<String, Self>;
    fn null() -> Self {
        Self::Null
    }
    fn boolean(value: bool) -> Self {
        Self::Bool(value)
    }
    fn number(value: Number) -> Self {
        Self::Number(value)
    }
    fn text(value: Text) -> Result<Self, ScanError> {
        Ok(Self::String(value))
    }
    fn array(values: Vec<Self>) -> Self {
        Self::Array(values)
    }
    fn object(values: Self::Map) -> Self {
        Self::Object(values)
    }
    fn contains(values: &Self::Map, key: &str) -> bool {
        values.contains_key(key)
    }
    fn insert(values: &mut Self::Map, key: String, value: Self) {
        values.insert(key, value);
    }
}
impl Build for Value {
    type Map = serde_json::Map<String, Self>;
    fn null() -> Self {
        Self::Null
    }
    fn boolean(value: bool) -> Self {
        Self::Bool(value)
    }
    fn number(value: Number) -> Self {
        Self::Number(value)
    }
    fn text(value: Text) -> Result<Self, ScanError> {
        value.into_string().map(Self::String)
    }
    fn array(values: Vec<Self>) -> Self {
        Self::Array(values)
    }
    fn object(values: Self::Map) -> Self {
        Self::Object(values)
    }
    fn contains(values: &Self::Map, key: &str) -> bool {
        values.contains_key(key)
    }
    fn insert(values: &mut Self::Map, key: String, value: Self) {
        values.insert(key, value);
    }
}

fn scan_into<R: Read, T: Build>(reader: R, limits: Limits) -> Result<(T, ScanStats), ScanError> {
    if limits.depth == 0 || limits.depth > HARD_DEPTH || limits.physical_bytes == u64::MAX {
        return Err(ScanError {
            kind: ErrorKind::InvalidLimits,
            offset: 0,
        });
    }
    let mut parser = Parser {
        input: Input {
            reader,
            bytes: [0; BUFFER],
            position: 0,
            filled: 0,
            offset: 0,
            maximum: limits.physical_bytes,
        },
        budget: Resident {
            maximum: limits.resident_bytes,
            used: 0,
            peak: 0,
        },
        stats: ScanStats::default(),
        limits,
    };
    parser.whitespace()?;
    let root = parser.node::<T>(0)?;
    parser.whitespace()?;
    if parser.input.peek()?.is_some() {
        return Err(parser.error(ErrorKind::Syntax));
    }
    parser.stats.physical_bytes = parser.input.offset;
    parser.stats.resident_bytes = parser.budget.used;
    parser.stats.peak_resident_bytes = parser.budget.peak;
    Ok((root, parser.stats))
}

struct Input<R> {
    reader: R,
    bytes: [u8; BUFFER],
    position: usize,
    filled: usize,
    offset: u64,
    maximum: u64,
}
impl<R: Read> Input<R> {
    fn peek(&mut self) -> Result<Option<u8>, ScanError> {
        if self.position == self.filled {
            let available = (self.maximum - self.offset)
                .saturating_add(1)
                .min(BUFFER as u64) as usize;
            self.filled = loop {
                match self.reader.read(&mut self.bytes[..available]) {
                    Ok(size) => break size,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        return Err(ScanError {
                            kind: ErrorKind::Io,
                            offset: self.offset,
                        });
                    }
                }
            };
            self.position = 0;
        }
        if self.position == self.filled {
            return Ok(None);
        }
        if self.offset == self.maximum {
            return Err(ScanError {
                kind: ErrorKind::PhysicalLimit,
                offset: self.offset,
            });
        }
        Ok(Some(self.bytes[self.position]))
    }
    fn next(&mut self) -> Result<Option<u8>, ScanError> {
        let byte = self.peek()?;
        if byte.is_some() {
            self.advance(1);
        }
        Ok(byte)
    }
    fn advance(&mut self, count: usize) {
        self.position += count;
        self.offset += count as u64;
    }
    fn chunk(&self) -> &[u8] {
        let count = (self.filled - self.position)
            .min((self.maximum - self.offset).min(BUFFER as u64) as usize);
        &self.bytes[self.position..self.position + count]
    }
}

struct Resident {
    maximum: usize,
    used: usize,
    peak: usize,
}
impl Resident {
    fn add(&mut self, count: usize, offset: u64) -> Result<(), ScanError> {
        let next = self
            .used
            .checked_add(count)
            .filter(|&next| next <= self.maximum)
            .ok_or(ScanError {
                kind: ErrorKind::ResidentLimit,
                offset,
            })?;
        self.used = next;
        self.peak = self.peak.max(next);
        Ok(())
    }
    fn release(&mut self, count: usize) {
        self.used -= count;
    }
    fn append(
        &mut self,
        string: &mut String,
        text: &str,
        limit: usize,
        offset: u64,
    ) -> Result<(), ScanError> {
        let length = string
            .len()
            .checked_add(text.len())
            .filter(|&length| length <= limit)
            .ok_or(ScanError {
                kind: ErrorKind::StringLimit,
                offset,
            })?;
        if length > string.capacity() {
            // Geometric growth is capped at the declared inline/key/number
            // bound. Charge before requesting an allocation.
            let capacity = length
                .max(string.capacity().saturating_mul(2).max(32))
                .min(limit);
            let before = string.capacity();
            self.add(capacity - before, offset)?;
            string
                .try_reserve_exact(capacity - string.len())
                .map_err(|_| ScanError {
                    kind: ErrorKind::ResidentLimit,
                    offset,
                })?;
            if string.capacity() > capacity {
                self.add(string.capacity() - capacity, offset)?;
            }
        }
        string.push_str(text);
        Ok(())
    }
}

// Hash decoded text in blocks even for adversarially escaped input. The small
// prefix is released immediately when it crosses the inline threshold.
struct StringBuild {
    inline: Option<String>,
    decoded_len: u64,
    hash: Option<Box<SpanHash>>,
}
impl StringBuild {
    fn new() -> Self {
        Self {
            inline: Some(String::new()),
            decoded_len: 0,
            hash: None,
        }
    }
    fn feed(
        &mut self,
        bytes: &[u8],
        threshold: usize,
        key: bool,
        budget: &mut Resident,
        offset: u64,
    ) -> Result<(), ScanError> {
        self.decoded_len += bytes.len() as u64;
        if self.decoded_len > threshold as u64 {
            if key {
                return Err(ScanError {
                    kind: ErrorKind::StringLimit,
                    offset,
                });
            }
            if let Some(inline) = self.inline.take() {
                // Ordinary keys/small strings never initialize the hash buffer
                // or compute a digest. A new span first hashes its ENTIRE
                // previously retained prefix, before releasing that prefix.
                budget.add(256, offset)?; // Retained decoded-prefix capacity.
                let mut hash = Box::new(SpanHash::new());
                hash.feed(inline.as_bytes());
                self.hash = Some(hash);
                budget.release(inline.capacity());
            }
        }
        if let Some(inline) = &mut self.inline {
            // Every feed is either a validated raw UTF-8 run or an encoded
            // Unicode scalar decoded from a valid JSON escape.
            let text = std::str::from_utf8(bytes).map_err(|_| ScanError {
                kind: ErrorKind::Utf8,
                offset,
            })?;
            budget.append(inline, text, threshold, offset)?;
        }
        if let Some(hash) = &mut self.hash {
            hash.feed(bytes);
        }
        Ok(())
    }
    fn finish(self, start: u64, end: u64, escaped: bool) -> Text {
        if let Some(inline) = self.inline {
            return Text::Inline(inline);
        }
        let hash = *self.hash.expect("span initialized a digest");
        Text::Span(TextSpan {
            start,
            end,
            escaped,
            decoded_len: self.decoded_len,
            digest: hash.full.finish(),
            prefix: hash.prefix,
            data_suffix: hash.suffix.map(|(offset, hash)| (offset, hash.finish())),
        })
    }
}
struct SpanHash {
    full: TextHash,
    prefix: Vec<u8>,
    seen: u64,
    suffix: Option<(u64, TextHash)>,
}
impl SpanHash {
    fn new() -> Self {
        Self {
            full: TextHash::new(),
            prefix: Vec::with_capacity(256),
            seen: 0,
            suffix: None,
        }
    }
    fn feed(&mut self, bytes: &[u8]) {
        self.full.feed(bytes);
        let count = bytes.len().min(256 - self.prefix.len());
        self.prefix.extend_from_slice(&bytes[..count]);
        if count != 0
            && self.suffix.is_none()
            && self.prefix.starts_with(b"data:")
            && let Some(comma) = self.prefix.iter().position(|byte| *byte == b',')
        {
            self.suffix = Some(((comma + 1) as u64, TextHash::new()));
        }
        if let Some((offset, hash)) = &mut self.suffix {
            let start = offset.saturating_sub(self.seen).min(bytes.len() as u64) as usize;
            hash.feed(&bytes[start..]);
        }
        self.seen += bytes.len() as u64;
    }
}
struct TextHash {
    hash: Sha1,
    pending: [u8; BUFFER],
    pending_len: usize,
}
impl TextHash {
    fn new() -> Self {
        Self {
            hash: Sha1::new(),
            pending: [0; BUFFER],
            pending_len: 0,
        }
    }
    fn feed(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            if self.pending_len == 0 && bytes.len() >= BUFFER {
                let count = bytes.len() / BUFFER * BUFFER;
                self.hash.update(&bytes[..count]);
                bytes = &bytes[count..];
            } else {
                let count = bytes.len().min(BUFFER - self.pending_len);
                self.pending[self.pending_len..self.pending_len + count]
                    .copy_from_slice(&bytes[..count]);
                self.pending_len += count;
                bytes = &bytes[count..];
                if self.pending_len == BUFFER {
                    self.hash.update(self.pending);
                    self.pending_len = 0;
                }
            }
        }
    }
    fn finish(mut self) -> [u8; 20] {
        self.hash.update(&self.pending[..self.pending_len]);
        self.hash.finalize().into()
    }
}

struct Parser<R> {
    input: Input<R>,
    budget: Resident,
    stats: ScanStats,
    limits: Limits,
}
impl<R: Read> Parser<R> {
    fn error(&self, kind: ErrorKind) -> ScanError {
        ScanError {
            kind,
            offset: self.input.offset,
        }
    }
    fn whitespace(&mut self) -> Result<(), ScanError> {
        while matches!(self.input.peek()?, Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.input.advance(1);
        }
        Ok(())
    }
    fn expect(&mut self, byte: u8) -> Result<(), ScanError> {
        if self.input.next()? != Some(byte) {
            return Err(self.error(ErrorKind::Syntax));
        }
        Ok(())
    }
    fn node<T: Build>(&mut self, depth: usize) -> Result<T, ScanError> {
        if self.stats.nodes >= self.limits.nodes {
            return Err(self.error(ErrorKind::NodeLimit));
        }
        self.stats.nodes += 1;
        self.budget.add(NODE_WEIGHT, self.input.offset)?;
        match self.input.peek()? {
            Some(b'n') => {
                for byte in b"null" {
                    self.expect(*byte)?;
                }
                Ok(T::null())
            }
            Some(b't') => {
                for byte in b"true" {
                    self.expect(*byte)?;
                }
                Ok(T::boolean(true))
            }
            Some(b'f') => {
                for byte in b"false" {
                    self.expect(*byte)?;
                }
                Ok(T::boolean(false))
            }
            Some(b'"') => T::text(self.string(false)?),
            Some(b'-' | b'0'..=b'9') => self.number().map(T::number),
            Some(b'[' | b'{') if depth >= self.limits.depth => {
                Err(self.error(ErrorKind::DepthLimit))
            }
            Some(b'[') => self.array::<T>(depth + 1),
            Some(b'{') => self.object::<T>(depth + 1),
            _ => Err(self.error(ErrorKind::Syntax)),
        }
    }
    fn array<T: Build>(&mut self, depth: usize) -> Result<T, ScanError> {
        self.expect(b'[')?;
        self.whitespace()?;
        let mut values = Vec::new();
        if self.input.peek()? == Some(b']') {
            self.input.advance(1);
            return Ok(T::array(values));
        }
        loop {
            values.push(self.node::<T>(depth)?);
            self.whitespace()?;
            match self.input.next()? {
                Some(b']') => return Ok(T::array(values)),
                Some(b',') => self.whitespace()?,
                _ => return Err(self.error(ErrorKind::Syntax)),
            }
        }
    }
    fn object<T: Build>(&mut self, depth: usize) -> Result<T, ScanError> {
        self.expect(b'{')?;
        self.whitespace()?;
        let mut values = T::Map::default();
        if self.input.peek()? == Some(b'}') {
            self.input.advance(1);
            return Ok(T::object(values));
        }
        loop {
            if self.stats.keys >= self.limits.keys {
                return Err(self.error(ErrorKind::KeyLimit));
            }
            self.stats.keys += 1;
            self.budget.add(KEY_WEIGHT, self.input.offset)?;
            let key_start = self.input.offset;
            let key = self.string(true)?.into_string()?;
            if T::contains(&values, &key) {
                return Err(ScanError {
                    kind: ErrorKind::DuplicateKey,
                    offset: key_start,
                });
            }
            self.whitespace()?;
            self.expect(b':')?;
            self.whitespace()?;
            let value = self.node::<T>(depth)?;
            T::insert(&mut values, key, value);
            self.whitespace()?;
            match self.input.next()? {
                Some(b'}') => return Ok(T::object(values)),
                Some(b',') => self.whitespace()?,
                _ => return Err(self.error(ErrorKind::Syntax)),
            }
        }
    }
    fn string(&mut self, key: bool) -> Result<Text, ScanError> {
        self.expect(b'"')?;
        let start = self.input.offset;
        let threshold = if key {
            self.limits.key_bytes
        } else {
            self.limits.inline_string_bytes
        };
        let mut build = StringBuild::new();
        let mut escaped = false;
        loop {
            match self.input.peek()? {
                Some(b'"') => {
                    let end = self.input.offset;
                    self.input.advance(1);
                    let text = build.finish(start, end, escaped);
                    if matches!(text, Text::Span(_)) {
                        self.stats.span_count += 1;
                    }
                    return Ok(text);
                }
                Some(b'\\') => {
                    escaped = true;
                    self.input.advance(1);
                    let character = match self.input.next()? {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{0008}',
                        Some(b'f') => '\u{000c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => self.unicode_escape()?,
                        _ => return Err(self.error(ErrorKind::Syntax)),
                    };
                    let mut bytes = [0; 4];
                    build.feed(
                        character.encode_utf8(&mut bytes).as_bytes(),
                        threshold,
                        key,
                        &mut self.budget,
                        self.input.offset,
                    )?;
                }
                Some(0..=0x1f) | None => return Err(self.error(ErrorKind::Syntax)),
                Some(_) => {
                    let chunk = self.input.chunk();
                    let end = chunk
                        .iter()
                        .position(|&byte| byte == b'"' || byte == b'\\' || byte < 0x20)
                        .unwrap_or(chunk.len());
                    let count = match std::str::from_utf8(&chunk[..end]) {
                        Ok(_) => end,
                        Err(error) if error.valid_up_to() != 0 => error.valid_up_to(),
                        Err(error) if error.error_len().is_some() => {
                            return Err(self.error(ErrorKind::Utf8));
                        }
                        Err(_) => 0,
                    };
                    if count > 0 {
                        build.feed(
                            &chunk[..count],
                            threshold,
                            key,
                            &mut self.budget,
                            self.input.offset,
                        )?;
                        self.input.advance(count);
                    } else {
                        // Only a scalar split by the fixed input buffer gets
                        // byte-wise treatment; ordinary/base64 runs stay bulk.
                        let first = self
                            .input
                            .next()?
                            .ok_or_else(|| self.error(ErrorKind::Syntax))?;
                        let width = match first {
                            0xc2..=0xdf => 2,
                            0xe0..=0xef => 3,
                            0xf0..=0xf4 => 4,
                            _ => return Err(self.error(ErrorKind::Utf8)),
                        };
                        let mut scalar = [0; 4];
                        scalar[0] = first;
                        for byte in &mut scalar[1..width] {
                            *byte = self
                                .input
                                .next()?
                                .ok_or_else(|| self.error(ErrorKind::Utf8))?;
                        }
                        std::str::from_utf8(&scalar[..width])
                            .map_err(|_| self.error(ErrorKind::Utf8))?;
                        build.feed(
                            &scalar[..width],
                            threshold,
                            key,
                            &mut self.budget,
                            self.input.offset,
                        )?;
                    }
                }
            }
        }
    }
    fn hex4(&mut self) -> Result<u16, ScanError> {
        let mut value = 0;
        for _ in 0..4 {
            let nibble = match self.input.next()? {
                Some(byte @ b'0'..=b'9') => byte - b'0',
                Some(byte @ b'a'..=b'f') => byte - b'a' + 10,
                Some(byte @ b'A'..=b'F') => byte - b'A' + 10,
                _ => return Err(self.error(ErrorKind::Syntax)),
            };
            value = value * 16 + nibble as u16;
        }
        Ok(value)
    }
    fn unicode_escape(&mut self) -> Result<char, ScanError> {
        let first = self.hex4()?;
        let scalar = match first {
            0xd800..=0xdbff => {
                self.expect(b'\\')?;
                self.expect(b'u')?;
                let second = self.hex4()?;
                if !(0xdc00..=0xdfff).contains(&second) {
                    return Err(self.error(ErrorKind::Syntax));
                }
                0x10000 + ((first as u32 - 0xd800) << 10) + (second as u32 - 0xdc00)
            }
            0xdc00..=0xdfff => return Err(self.error(ErrorKind::Syntax)),
            _ => first as u32,
        };
        char::from_u32(scalar).ok_or_else(|| self.error(ErrorKind::Syntax))
    }
    fn number_byte(&mut self, token: &mut String) -> Result<(), ScanError> {
        if token.len() >= self.limits.number_bytes {
            return Err(self.error(ErrorKind::NumberLimit));
        }
        let byte = self
            .input
            .next()?
            .ok_or_else(|| self.error(ErrorKind::Syntax))?;
        let buffer = [byte];
        self.budget.append(
            token,
            std::str::from_utf8(&buffer).map_err(|_| self.error(ErrorKind::Syntax))?,
            self.limits.number_bytes,
            self.input.offset,
        )
    }
    fn digits(&mut self, token: &mut String) -> Result<(), ScanError> {
        if !matches!(self.input.peek()?, Some(b'0'..=b'9')) {
            return Err(self.error(ErrorKind::Syntax));
        }
        while matches!(self.input.peek()?, Some(b'0'..=b'9')) {
            self.number_byte(token)?;
        }
        Ok(())
    }
    fn number(&mut self) -> Result<Number, ScanError> {
        let mut token = String::new();
        if self.input.peek()? == Some(b'-') {
            self.number_byte(&mut token)?;
        }
        match self.input.peek()? {
            Some(b'0') => self.number_byte(&mut token)?,
            Some(b'1'..=b'9') => self.digits(&mut token)?,
            _ => return Err(self.error(ErrorKind::Syntax)),
        }
        if self.input.peek()? == Some(b'.') {
            self.number_byte(&mut token)?;
            self.digits(&mut token)?;
        }
        if matches!(self.input.peek()?, Some(b'e' | b'E')) {
            self.number_byte(&mut token)?;
            if matches!(self.input.peek()?, Some(b'+' | b'-')) {
                self.number_byte(&mut token)?;
            }
            self.digits(&mut token)?;
        }
        let value = serde_json::from_str(&token).map_err(|_| self.error(ErrorKind::Syntax))?;
        self.budget.release(token.capacity());
        Ok(value)
    }
}

#[cfg(test)]
mod tests;
