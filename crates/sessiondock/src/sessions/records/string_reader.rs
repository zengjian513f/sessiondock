//! A bounded decoder for the physical INSIDE of one JSON string (no quotes).
//! No path access, media classification or authorization lives here. The caller
//! must retain/finish its checked source and discard output on any read error.
use sha1::{Digest, Sha1};
use std::io::{self, Read};

const BUFFER: usize = 8192;
const MAX_PHYSICAL: u64 = 256 * 1024 * 1024;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid native JSON string")
}
fn incomplete() -> io::Error {
    io::Error::other("native JSON string not completely verified")
}

/// Emits unescaped UTF-8, never a resident copy of the entire input string.
/// Successful EOF verifies the expected decoded length and SHA-1. Output before
/// EOF is provisional; `finish` is mandatory before publishing/materializing it.
pub(crate) struct JsonStringReader<R: Read> {
    reader: R,
    input: [u8; BUFFER],
    position: usize,
    available: usize,
    physical: u64,
    physical_limit: u64,
    pending: [u8; 4],
    pending_position: usize,
    pending_length: usize,
    expected_len: u64,
    expected_sha1: [u8; 20],
    decoded: u64,
    hash: Sha1,
    eof: bool,
    verified: bool,
    failed: bool,
}

impl<R: Read> JsonStringReader<R> {
    pub(crate) fn new(
        reader: R,
        expected_decoded_len: u64,
        expected_sha1: [u8; 20],
        physical_limit: u64,
    ) -> io::Result<Self> {
        if physical_limit > MAX_PHYSICAL || expected_decoded_len > physical_limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native JSON string exceeds operation budget",
            ));
        }
        Ok(Self {
            reader,
            input: [0; BUFFER],
            position: 0,
            available: 0,
            physical: 0,
            physical_limit,
            pending: [0; 4],
            pending_position: 0,
            pending_length: 0,
            expected_len: expected_decoded_len,
            expected_sha1,
            decoded: 0,
            hash: Sha1::new(),
            eof: false,
            verified: false,
            failed: false,
        })
    }

    /// Does not drain automatically: a consumer which stopped early cannot
    /// claim completion. Return the original reader for its own final checks.
    pub(crate) fn finish(self) -> io::Result<R> {
        if self.failed || !self.verified {
            return Err(incomplete());
        }
        Ok(self.reader)
    }

    fn refill(&mut self) -> io::Result<bool> {
        if self.position < self.available {
            return Ok(true);
        }
        if self.eof {
            return Ok(false);
        }
        // A single byte beyond an exact maximum distinguishes EOF from an
        // oversized source. A checked range itself never reads past its end.
        let amount = (self.physical_limit - self.physical)
            .min(BUFFER as u64)
            .max(1) as usize;
        let count = loop {
            match self.reader.read(&mut self.input[..amount]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(io::Error::other("native JSON string source read failed")),
                Ok(count) => break count,
            }
        };
        if count as u64 > self.physical_limit - self.physical {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native JSON string exceeds operation budget",
            ));
        }
        self.physical += count as u64;
        self.position = 0;
        self.available = count;
        self.eof = count == 0;
        Ok(!self.eof)
    }

    fn required(&mut self) -> io::Result<u8> {
        if !self.refill()? {
            return Err(invalid());
        }
        let byte = self.input[self.position];
        self.position += 1;
        Ok(byte)
    }

    fn hex_quad(&mut self) -> io::Result<u16> {
        let mut value = 0;
        for _ in 0..4 {
            let byte = self.required()?;
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(invalid()),
            };
            value = (value << 4) | u16::from(digit);
        }
        Ok(value)
    }

    fn escaped(&mut self) -> io::Result<()> {
        let byte = self.required()?;
        let scalar = match byte {
            b'"' | b'\\' | b'/' => char::from(byte),
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => {
                let first = self.hex_quad()?;
                let code = if (0xd800..=0xdbff).contains(&first) {
                    if self.required()? != b'\\' || self.required()? != b'u' {
                        return Err(invalid());
                    }
                    let second = self.hex_quad()?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(invalid());
                    }
                    0x10000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
                } else {
                    u32::from(first)
                };
                char::from_u32(code).ok_or_else(invalid)?
            }
            _ => return Err(invalid()),
        };
        self.pending_length = scalar.encode_utf8(&mut self.pending).len();
        self.pending_position = 0;
        Ok(())
    }

    fn raw_scalar(&mut self, first: u8) -> io::Result<()> {
        let length = match first {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return Err(invalid()),
        };
        self.pending[0] = first;
        for index in 1..length {
            self.pending[index] = self.required()?;
        }
        // Reject invalid continuations, overlong sequences, surrogate scalars
        // and values above U+10FFFF, including across physical read boundaries.
        std::str::from_utf8(&self.pending[..length]).map_err(|_| invalid())?;
        self.pending_position = 0;
        self.pending_length = length;
        Ok(())
    }

    fn read_inner(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let mut written = 0;
        while written < output.len() {
            if self.pending_position < self.pending_length {
                let count =
                    (self.pending_length - self.pending_position).min(output.len() - written);
                output[written..written + count].copy_from_slice(
                    &self.pending[self.pending_position..self.pending_position + count],
                );
                self.pending_position += count;
                written += count;
                continue;
            }
            if !self.refill()? {
                break;
            }
            let remaining = &self.input[self.position..self.available];
            let count = remaining
                .iter()
                .take(output.len() - written)
                .take_while(|byte| matches!(**byte, 0x20..=0x7f) && !matches!(**byte, b'"' | b'\\'))
                .count();
            if count > 0 {
                output[written..written + count].copy_from_slice(&remaining[..count]);
                self.position += count;
                written += count;
                continue;
            }
            let first = self.required()?;
            match first {
                b'\\' => self.escaped()?,
                0x80..=0xff => self.raw_scalar(first)?,
                _ => return Err(invalid()), // Raw quote/control is never inside a JSON string.
            }
        }
        Ok(written)
    }
}

impl<R: Read> Read for JsonStringReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.failed {
            return Err(incomplete());
        }
        if output.is_empty() {
            return Ok(0);
        }
        let result = (|| {
            let count = self.read_inner(output)?;
            self.decoded += count as u64;
            if self.decoded > self.expected_len {
                return Err(invalid());
            }
            self.hash.update(&output[..count]);
            if self.eof {
                if self.decoded != self.expected_len
                    || <[u8; 20]>::from(self.hash.clone().finalize()) != self.expected_sha1
                {
                    return Err(invalid());
                }
                self.verified = true;
            }
            Ok(count)
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

#[cfg(test)]
mod tests;
