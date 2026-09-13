//! Search-text production: where the body of one candidate comes from, in
//! this order — the persistent cache at the candidate's current version, a
//! still-current view in the LRU (borrowed, then remembered), else a
//! transient projection under the shared parse-slot budget, matched and
//! dropped (never retained: keeping decoded records resident to decode only
//! appended bytes would cost several times the file per active session,
//! against the read model's memory rule; Python re-reads too). One producer
//! per uid; a second search waits and re-reads the cache. The warm-up walks
//! the same path at background priority (docs/read-model.md "搜索").

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use super::{
    Outcome, PreparedSearch, Scanned, Scanner, SearchError, body,
    cache::{Cached, Hit, Lookup, ParseSlots, Priority, SearchCache, TextReader},
};
use crate::sessions::{SearchPool, SessionError, SessionStore};

/// Bytes of cached bodies that may be held in memory whole at once, for the
/// queries that cannot be matched chunk by chunk.
pub const WHOLE_BODY_BUDGET: u64 = 64 * 1024 * 1024;
const WHOLE_BODY_UNIT: u64 = 8 * 1024 * 1024;
const WARMUP_DELAY: Duration = Duration::from_secs(2);

/// Hand freed heap back to the OS after parses: transient projections leave
/// glibc arenas fragmented, and a search must not raise the resident set for
/// good. glibc only; a no-op elsewhere (and harmless under another global
/// allocator, whose memory it does not manage).
pub fn release_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        // SAFETY: glibc's malloc_trim takes no pointers and is thread-safe.
        unsafe {
            malloc_trim(0);
        }
    }
}

fn cancelled_error() -> SessionError {
    SessionError {
        status: 499,
        message: "搜索已取消".to_owned(),
    }
}

/// The body of one candidate for matching.
pub enum Source {
    Text(TextReader),
    Error { status: u16, message: String },
}

impl From<Hit> for Source {
    fn from(hit: Hit) -> Self {
        match hit {
            Hit::Text(reader) => Source::Text(reader),
            Hit::Error { status, message } => Source::Error { status, message },
        }
    }
}

impl From<Cached> for Source {
    fn from(cached: Cached) -> Self {
        match cached {
            Cached::Text(text) => Source::Text(TextReader::memory(text)),
            Cached::Error { status, message } => Source::Error { status, message },
        }
    }
}

/// One warm-up pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WarmReport {
    pub candidates: usize,
    pub hits: usize,
    pub produced: usize,
    pub failed: usize,
    pub elapsed_ms: u128,
}

pub struct SearchService {
    pub store: Arc<SessionStore>,
    pub cache: Arc<SearchCache>,
    pub slots: Arc<ParseSlots>,
    whole_reads: ParseSlots,
    pub workers: usize,
    pub warmup_secs: u64,
    /// Bodies produced (parsed) so far; a search that moved it trims the heap.
    produced: AtomicUsize,
}

impl SearchService {
    pub fn open(
        store: Arc<SessionStore>,
        cache_dir: Option<std::path::PathBuf>,
        cache_bytes: u64,
        workers: usize,
        warmup_secs: u64,
    ) -> std::io::Result<Self> {
        Ok(Self {
            store,
            cache: Arc::new(SearchCache::open(cache_dir, cache_bytes)?),
            slots: Arc::new(ParseSlots::new(workers)),
            whole_reads: ParseSlots::with_unit(
                (WHOLE_BODY_BUDGET / WHOLE_BODY_UNIT) as usize,
                WHOLE_BODY_UNIT,
            ),
            workers: workers.max(1),
            warmup_secs,
            produced: AtomicUsize::new(0),
        })
    }

    /// Number of bodies parsed so far (monotonic).
    pub fn produced(&self) -> usize {
        self.produced.load(Ordering::Relaxed)
    }

    /// The body of `uid` at its current version, and whether it had to be
    /// produced (parsed) rather than found.
    pub fn source(
        &self,
        pool: &SearchPool,
        uid: &str,
        priority: Priority,
        cancelled: &AtomicBool,
    ) -> Result<(Source, bool), SessionError> {
        let version = self.store.search_version(pool, uid)?;
        if let Lookup::Hit(hit) = self.cache.get(uid, &version) {
            return Ok((hit.into(), false));
        }
        // A view the LRU still holds for exactly these files costs nothing.
        if let Some(view) = self.store.search_view_cached(pool, uid)? {
            let text = body(&view);
            drop(view);
            let cached = Cached::Text(text);
            if self.store.search_version(pool, uid)? == version {
                self.cache.put(uid, &version, &cached);
            }
            return Ok((cached.into(), false));
        }
        let _producer = self.cache.inflight(uid);
        if let Lookup::Hit(hit) = self.cache.get(uid, &version) {
            return Ok((hit.into(), false));
        }
        let bytes = version.data.as_ref().map_or(0, |(_, size)| *size);
        let Some(_slot) = self
            .slots
            .acquire(self.slots.weight(bytes), priority, cancelled)
        else {
            return Err(cancelled_error());
        };
        let cached = match self.store.search_view_transient(pool, uid) {
            Ok(view) => {
                // The view (and any transient projection behind it) lives
                // only for this body; the text is the only retained part.
                let text = body(&view);
                drop(view);
                Cached::Text(text)
            }
            Err(error) if Cached::cacheable_error(error.status) => Cached::Error {
                status: error.status,
                message: error.message,
            },
            Err(error) => return Err(error),
        };
        // Never persist a body under a version that changed while it was
        // being read; the next search parses once more.
        if self.store.search_version(pool, uid)? == version {
            self.cache.put(uid, &version, &cached);
        }
        let produced = self.produced.fetch_add(1, Ordering::Relaxed) + 1;
        // Each worker thread's arena keeps the high-water mark of its largest
        // projection until trimmed; without this a cold pass over many
        // sessions sits at (threads × largest parse) of freed-but-resident
        // heap. Trim after every large parse and every 32 small ones.
        if bytes >= super::cache::SLOT_BYTES || produced.is_multiple_of(32) {
            release_memory();
        }
        Ok((cached.into(), true))
    }

    /// Match one candidate for a search request: cached bodies stream through
    /// `buffer` chunk by chunk; a query that may match across newlines reads
    /// the body whole under the whole-body budget.
    pub fn scan(
        &self,
        pool: &SearchPool,
        uid: &str,
        query: &PreparedSearch,
        cancelled: &AtomicBool,
        buffer: &mut Vec<u8>,
    ) -> Scanned {
        let reader = match self.source(pool, uid, Priority::Foreground, cancelled) {
            Ok((Source::Text(reader), _)) => reader,
            Ok((Source::Error { status, message }, _)) => {
                return Scanned::Error(SessionError { status, message });
            }
            Err(error) => return Scanned::Error(error),
        };
        match self.matches(reader, uid, query, cancelled, buffer) {
            Ok(outcome) => Scanned::Matched(outcome),
            Err(error) => Scanned::Error(SessionError {
                status: error.status,
                message: error.message,
            }),
        }
    }

    fn matches(
        &self,
        reader: TextReader,
        uid: &str,
        query: &PreparedSearch,
        cancelled: &AtomicBool,
        buffer: &mut Vec<u8>,
    ) -> Result<Outcome, SearchError> {
        let mut scanner = Scanner::new(query);
        if query.chunkable() {
            let result = reader.for_each_chunk(buffer, |chunk, last| {
                scanner.feed(chunk, last, cancelled)?;
                Ok(scanner.wants_more())
            });
            if let Err(error) = result {
                if error.status != 499 {
                    self.cache.remove(uid);
                }
                return Err(error);
            }
            return Ok(scanner.finish());
        }
        let Some(_budget) = self.whole_reads.acquire(
            self.whole_reads.weight(reader.len()),
            Priority::Foreground,
            cancelled,
        ) else {
            return Err(SearchError::cancelled());
        };
        let text = reader.read_all().map_err(|error| {
            self.cache.remove(uid);
            SearchError::new(503, "session_error", error.to_string())
        })?;
        scanner.feed(&text, true, cancelled)?;
        Ok(scanner.finish())
    }

    /// One pass over every candidate at background priority: cache hits are
    /// only stat + header checks; misses are produced under the parse
    /// budget with `threads` producers that yield to searches.
    pub fn warm(
        &self,
        shutdown: &tokio_util::sync::CancellationToken,
        threads: usize,
    ) -> Result<WarmReport, SessionError> {
        let started = Instant::now();
        let pool = self.store.search_pool()?;
        let never = AtomicBool::new(false);
        let next = AtomicUsize::new(0);
        let (hits, produced, failed) = (
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        );
        let threads = threads.clamp(1, pool.rows.len().max(1));
        std::thread::scope(|scope| {
            for _ in 0..threads {
                let (pool, next, never) = (&pool, &next, &never);
                let (hits, produced, failed) = (&hits, &produced, &failed);
                scope.spawn(move || {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= pool.rows.len() || shutdown.is_cancelled() {
                            break;
                        }
                        let uid = pool.rows[index]["uid"].as_str().unwrap_or("");
                        match self.source(pool, uid, Priority::Background, never) {
                            Ok((_, true)) => produced.fetch_add(1, Ordering::Relaxed),
                            Ok((_, false)) => hits.fetch_add(1, Ordering::Relaxed),
                            Err(_) => failed.fetch_add(1, Ordering::Relaxed),
                        };
                    }
                });
            }
        });
        let report = WarmReport {
            candidates: pool.rows.len(),
            hits: hits.into_inner(),
            produced: produced.into_inner(),
            failed: failed.into_inner(),
            elapsed_ms: started.elapsed().as_millis(),
        };
        if report.produced > 0 || report.failed > 0 {
            release_memory();
        }
        Ok(report)
    }

    /// The background warm-up thread: a first pass shortly after start with
    /// every parse slot (a cold cache is the one time speed matters), then
    /// one every `warmup_secs` with half of them. Only with a persistent
    /// cache; nothing to warm otherwise. Stops with the shutdown token.
    pub fn spawn_warmup(self: &Arc<Self>, shutdown: tokio_util::sync::CancellationToken) -> bool {
        if self.warmup_secs == 0 || !self.cache.persistent() {
            return false;
        }
        let service = self.clone();
        let interval = Duration::from_secs(self.warmup_secs);
        let spawned = std::thread::Builder::new()
            .name("search-warmup".to_owned())
            .spawn(move || {
                let mut delay = WARMUP_DELAY;
                let mut threads = service.workers;
                loop {
                    let deadline = Instant::now() + delay;
                    while Instant::now() < deadline {
                        if shutdown.is_cancelled() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    if shutdown.is_cancelled() {
                        return;
                    }
                    match service.warm(&shutdown, threads) {
                        Ok(report) if report.produced > 0 || report.failed > 0 => {
                            let stats = service.cache.stats();
                            eprintln!(
                                "search-text warm-up: {} candidates, {} cached, {} produced, {} failed, {} ms; cache {} entries / {} bytes",
                                report.candidates, report.hits, report.produced, report.failed,
                                report.elapsed_ms, stats.entries, stats.bytes
                            );
                        }
                        Ok(_) => {}
                        Err(error) => eprintln!("search-text warm-up skipped: {}", error.message),
                    }
                    delay = interval;
                    threads = (service.workers / 2).max(1);
                }
            });
        spawned.is_ok()
    }
}
