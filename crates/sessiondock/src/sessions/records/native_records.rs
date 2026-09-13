//! Pull-based complete-record scanning. Large strings stay private spans; the
//! index observes the physical input before any provider interpretation.
use super::*;
use crate::media::{NativeImage, NativeSpan};
use std::io::{self, BufRead, BufReader, Cursor, Read, Write};

const SMALL: usize = 64 * 1024;
const OVERSIZED: &str = "普通原生记录超过 64 MiB 上限";
mod replay_source;

/// Why a complete line produced no row: not a JSON object (skipped and
/// counted like the Python adapters), or a record budget / media failure
/// that still fails the session (KEEP).
enum Rejected {
    Invalid,
    Hard(String),
}

struct Observed<'a, R> {
    reader: R,
    index: &'a mut super::super::native_input::RawIndexBuilder,
    failure: &'a mut Option<SessionError>,
}
impl<R: Read> Read for Observed<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(error) = self.failure.as_ref() {
            return Err(io::Error::other(error.clone()));
        }
        let count = loop {
            match self.reader.read(buffer) {
                Ok(count) => break count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let error = io_error(error);
                    *self.failure = Some(error.clone());
                    return Err(io::Error::other(error));
                }
            }
        };
        if let Err(error) = self.index.push(&buffer[..count]) {
            *self.failure = Some(error.clone());
            return Err(io::Error::other(error));
        }
        Ok(count)
    }
}
struct Line<'a, R> {
    reader: &'a mut R,
    consumed: u64,
    complete: bool,
}
impl<R: BufRead> Read for Line<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.complete || buffer.is_empty() {
            return Ok(0);
        }
        let available = self.reader.fill_buf()?;
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |at| at + 1)
            .min(buffer.len());
        buffer[..count].copy_from_slice(&available[..count]);
        self.complete = count > 0 && buffer[count - 1] == b'\n';
        self.reader.consume(count);
        self.consumed += count as u64;
        Ok(count)
    }
}
fn io_error(error: io::Error) -> SessionError {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<SessionError>())
        .cloned()
        .unwrap_or_else(|| SessionError::new(503, "原生记录读取失败，未发布不完整快照"))
}

// Discharging an image span must not also admit an arbitrarily large ordinary
// body. Count the remaining serialized JSON without making another body copy.
fn ordinary_body_fits(value: &Value) -> bool {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            if self.0 > LINE_LIMIT {
                return Err(io::Error::other("ordinary record limit"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(0), value).is_ok()
}

pub(in crate::sessions) fn scan_native_records(
    reader: impl Read,
    decoder: &mut Decoder,
    probe: Option<u64>,
    maximum: u64,
    candidate: &Candidate,
) -> Result<RawIndex, SessionError> {
    let mut index = super::super::native_input::RawIndexBuilder::new(
        super::super::native_input::IndexLimits {
            max_bytes: maximum,
            ..Default::default()
        },
        probe,
    )?;
    let mut failure = None;
    let result = (|| -> Result<(), SessionError> {
        let mut reader = BufReader::with_capacity(
            SMALL,
            Observed {
                reader,
                index: &mut index,
                failure: &mut failure,
            },
        );
        // Skipping AST decode is not skipping the raw index/hash. A stale skip
        // boundary is never accepted until the full old-prefix probe matches.
        let skipped =
            io::copy(&mut reader.by_ref().take(decoder.skip), &mut io::sink()).map_err(io_error)?;
        decoder.offset = skipped;
        loop {
            if reader.fill_buf().map_err(io_error)?.is_empty() {
                break;
            }
            let start = decoder.offset;
            let mut line = Line {
                reader: &mut reader,
                consumed: 0,
                complete: false,
            };
            if decoder.error.is_some() {
                io::copy(&mut line, &mut io::sink()).map_err(io_error)?;
                decoder.offset = start + line.consumed;
                continue;
            }
            let mut prefix = Vec::with_capacity(SMALL);
            line.by_ref()
                .take(SMALL as u64)
                .read_to_end(&mut prefix)
                .map_err(io_error)?;
            if prefix.iter().all(u8::is_ascii_whitespace) && line.complete {
                decoder.offset = start + line.consumed;
                continue;
            }
            let decoded = if line.complete {
                decode_record(&prefix)
                    .map(|value| (value, Vec::new()))
                    .map_err(|_| Rejected::Invalid)
            } else {
                let input = Cursor::new(prefix).chain(&mut line);
                let document = scanner::scan(input, record_limits(maximum));
                // Invalid JSON must still be drained through the physical LF.
                // EOF without LF never turns a partial record into a row/error.
                io::copy(&mut line, &mut io::sink()).map_err(io_error)?;
                let end = start + line.consumed;
                match document {
                    // An uncommitted tail is neither a row nor a skipped line.
                    _ if !line.complete => Err(Rejected::Invalid),
                    Ok(document) if document.stats.span_count > 0 => {
                        // Image spans leave the record; ordinary giant text
                        // is read back, so a record without any image stays
                        // bound by the same physical line budget as the
                        // small-record path.
                        match replay_source::prepare(candidate, start, end, document.root)? {
                            Ok((_, sidecars))
                                if sidecars.iter().all(|sidecar| sidecar.image.is_none())
                                    && line.consumed > LINE_LIMIT as u64 =>
                            {
                                Err(Rejected::Hard(OVERSIZED.into()))
                            }
                            Ok(prepared) => Ok(prepared),
                            Err(reason) => Err(Rejected::Hard(reason)),
                        }
                    }
                    _ if line.consumed > LINE_LIMIT as u64 => Err(Rejected::Hard(OVERSIZED.into())),
                    Ok(document) => document
                        .into_value()
                        .map(|value| (value, Vec::new()))
                        .map_err(|_| Rejected::Invalid),
                    Err(_) => Err(Rejected::Invalid),
                }
            };
            decoder.offset = start + line.consumed;
            if !line.complete {
                break;
            }
            #[cfg(test)]
            {
                decoder.decoded += 1;
            }
            match decoded {
                Ok((row, sidecars)) if row.is_object() => {
                    // Saturating accounting; the cache applies its budget at
                    // `retain` (batch 44 WP-A: budgets are per cache, not global).
                    decoder.weight = decoder
                        .weight
                        .saturating_add(value_weight(&row))
                        .saturating_add(32);
                    for sidecar in &sidecars {
                        decoder.weight = decoder.weight.saturating_add(sidecar.retained_weight());
                    }
                    if !sidecars.is_empty() {
                        decoder.weight = decoder
                            .weight
                            .saturating_add(
                                (sidecars.capacity() - sidecars.len())
                                    .saturating_mul(std::mem::size_of::<native_images::Sidecar>()),
                            )
                            .saturating_add(128); // outer ordered-map slot/bookkeeping
                    }
                    if !sidecars.is_empty() {
                        decoder.sidecars.insert(decoder.offset, sidecars);
                    }
                    decoder.records.push((row, decoder.offset));
                }
                Ok(_) | Err(Rejected::Invalid) => decoder.invalid += 1,
                Err(Rejected::Hard(reason)) => decoder.error = Some(reason),
            }
            if decoder.records.len() > ROW_LIMIT {
                decoder.error = Some("原生记录超过 1000000 条限制".into());
            }
        }
        Ok(())
    })();
    if let Some(error) = failure {
        return Err(error);
    }
    result?;
    index.finish()
}

#[cfg(test)]
mod tests;
