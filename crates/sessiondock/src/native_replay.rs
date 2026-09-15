//! Checked-source-independent replay of nested JSON string interiors.
//! Plans describe ranges, never file authority. Each parent is completed and
//! verified even when the requested child ended much earlier in that parent.
use crate::fingerprint::Digest;
use crate::sessions::JsonStringReader;
use std::io::{self, Read};

const CHUNK: usize = 8192;

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid native replay plan or content",
    )
}
fn incomplete() -> io::Error {
    io::Error::other("native replay was not completely verified")
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StringRange {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) decoded_len: u64,
    pub(crate) decoded_digest: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DecodePlan {
    ranges: Box<[StringRange]>,
}
impl DecodePlan {
    pub(crate) fn new(ranges: Vec<StringRange>) -> io::Result<Self> {
        if ranges.is_empty() {
            return Err(invalid());
        }
        for (index, range) in ranges.iter().enumerate() {
            let physical = range.end.checked_sub(range.start).ok_or_else(invalid)?;
            if range.decoded_len > physical
                || (index > 0 && range.end > ranges[index - 1].decoded_len)
            {
                return Err(invalid());
            }
        }
        Ok(Self {
            ranges: ranges.into_boxed_slice(),
        })
    }
    pub(crate) fn ranges(&self) -> &[StringRange] {
        &self.ranges
    }
    pub(crate) fn first(&self) -> &StringRange {
        &self.ranges[0]
    }
    pub(crate) fn last(&self) -> &StringRange {
        self.ranges.last().expect("nonempty validated plan")
    }
    /// Translate only the physical outer range; child offsets address decoded
    /// parent text and must never receive a native-file byte offset.
    pub(crate) fn with_outer_offset(&self, offset: u64) -> io::Result<Self> {
        let mut ranges = self.ranges.to_vec();
        ranges[0].start = ranges[0].start.checked_add(offset).ok_or_else(invalid)?;
        ranges[0].end = ranges[0].end.checked_add(offset).ok_or_else(invalid)?;
        Self::new(ranges)
    }
    /// Includes inline plan structure plus exact boxed heap storage.
    pub(crate) fn resident_len(&self) -> usize {
        std::mem::size_of::<Self>() + std::mem::size_of_val(self.ranges())
    }
}

enum Stage<R: Read> {
    Source {
        reader: R,
    },
    String {
        reader: Box<JsonStringReader<Window<R>>>,
    },
}
struct Window<R: Read> {
    parent: Box<Stage<R>>,
    skip: u64,
    remaining: u64,
}
impl<R: Read> Read for Window<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let mut discarded = [0; CHUNK];
        while self.skip > 0 {
            let amount = self.skip.min(CHUNK as u64) as usize;
            let count = self.parent.read(&mut discarded[..amount])?;
            if count == 0 {
                return Err(invalid());
            }
            self.skip -= count as u64;
        }
        if self.remaining == 0 {
            return Ok(0);
        }
        let amount = self.remaining.min(output.len().min(CHUNK) as u64) as usize;
        let count = self.parent.read(&mut output[..amount])?;
        if count == 0 {
            return Err(invalid());
        }
        self.remaining -= count as u64;
        Ok(count)
    }
}
impl<R: Read> Read for Stage<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Source { reader } => {
                let amount = output.len().min(CHUNK);
                loop {
                    match reader.read(&mut output[..amount]) {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => return Err(io::Error::other("native replay source read failed")),
                        result => return result,
                    }
                }
            }
            Self::String { reader } => {
                let amount = output.len().min(CHUNK);
                reader.read(&mut output[..amount])
            }
        }
    }
}
impl<R: Read> Stage<R> {
    fn finish(self) -> io::Result<R> {
        match self {
            Self::Source { reader } => Ok(reader),
            Self::String { reader } => {
                let window = (*reader).finish()?;
                if window.skip != 0 || window.remaining != 0 {
                    return Err(incomplete());
                }
                let mut parent = *window.parent;
                if matches!(&parent, Self::Source { .. }) {
                    // The caller promised an EXACT outer range, not a prefix
                    // of an arbitrary larger reader. Do not silently accept it.
                    if parent.read(&mut [0; 1])? != 0 {
                        return Err(invalid());
                    }
                } else {
                    let mut discarded = [0; CHUNK];
                    while parent.read(&mut discarded)? != 0 {}
                }
                parent.finish()
            }
        }
    }
}

pub(crate) struct ReplayReader<R: Read> {
    stage: Stage<R>,
    eof: bool,
    failed: bool,
}
impl<R: Read> ReplayReader<R> {
    /// `reader` is already authorized and confined to first.start..first.end;
    /// its current offset is zero. No seek/path opening happens here.
    pub(crate) fn new(reader: R, plan: &DecodePlan) -> io::Result<Self> {
        let mut stage = Stage::Source { reader };
        for (index, range) in plan.ranges().iter().enumerate() {
            let window = Window {
                parent: Box::new(stage),
                skip: if index == 0 { 0 } else { range.start },
                remaining: range.end - range.start,
            };
            let reader = JsonStringReader::new(
                window,
                range.decoded_len,
                range.decoded_digest,
                range.end - range.start,
            )?;
            stage = Stage::String {
                reader: Box::new(reader),
            };
        }
        Ok(Self {
            stage,
            eof: false,
            failed: false,
        })
    }
    /// Final layer EOF is mandatory before finish. Finish then drains every
    /// parent tail, validates each parent digest, and returns the original R
    /// for its checked-handle completion. It never silently drains the target.
    pub(crate) fn finish(self) -> io::Result<R> {
        if self.failed || !self.eof {
            return Err(incomplete());
        }
        self.stage.finish()
    }
}
impl<R: Read> Read for ReplayReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.failed {
            return Err(incomplete());
        }
        if output.is_empty() {
            return Ok(0);
        }
        let result = self.stage.read(output);
        if result.is_err() {
            self.failed = true;
        }
        if result.as_ref().is_ok_and(|count| *count == 0) {
            self.eof = true;
        }
        result
    }
}

pub(crate) trait CheckedReplay: Read {
    fn finish(self: Box<Self>) -> Result<(), String>;
}

#[cfg(test)]
mod tests;
