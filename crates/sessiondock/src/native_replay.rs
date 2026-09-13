//! Checked-source-independent replay of nested JSON string interiors.
//! Plans describe ranges, never file authority. Each parent is completed and
//! verified even when the requested child ended much earlier in that parent.
use crate::sessions::JsonStringReader;
use std::{
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const MAX_LAYERS: usize = 9;
const MAX_RANGE: u64 = 256 * 1024 * 1024;
const DEFAULT_WORK: u64 = 512 * 1024 * 1024;
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
fn budget_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "native replay work budget exceeded",
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StringRange {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) decoded_len: u64,
    pub(crate) decoded_sha1: [u8; 20],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DecodePlan {
    ranges: Box<[StringRange]>,
}
impl DecodePlan {
    pub(crate) fn new(ranges: Vec<StringRange>) -> io::Result<Self> {
        if ranges.is_empty() || ranges.len() > MAX_LAYERS {
            return Err(invalid());
        }
        for (index, range) in ranges.iter().enumerate() {
            let physical = range.end.checked_sub(range.start).ok_or_else(invalid)?;
            if physical > MAX_RANGE
                || range.decoded_len > physical
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
    /// Includes inline plan structure plus exact boxed heap storage. A parent
    /// already charging its own size should add only the ranges' heap bytes.
    pub(crate) fn resident_len(&self) -> usize {
        std::mem::size_of::<Self>() + std::mem::size_of_val(self.ranges())
    }
}

struct Work {
    maximum: u64,
    used: AtomicU64,
    failed: AtomicBool,
}
/// Shared across every layer, candidate and reopen in one caller-owned
/// operation. Charges are irreversible; cloning never replenishes the budget.
#[derive(Clone)]
pub(crate) struct WorkBudget(Arc<Work>);
impl WorkBudget {
    pub(crate) fn new(maximum: u64) -> Self {
        Self(Arc::new(Work {
            maximum,
            used: AtomicU64::new(0),
            failed: AtomicBool::new(false),
        }))
    }
    pub(crate) fn charge(&self, bytes: u64) -> io::Result<()> {
        self.check()?;
        let result = self
            .0
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.0.maximum)
            });
        if result.is_err() {
            self.0.failed.store(true, Ordering::Release);
            return Err(budget_error());
        }
        self.check()
    }
    pub(crate) fn remaining(&self) -> u64 {
        if self.exhausted() {
            0
        } else {
            self.0.maximum - self.0.used.load(Ordering::Acquire)
        }
    }
    /// True only after a failed charge (exactly used-up budget may still verify
    /// zero-byte EOF). Once true it cannot be cleared, including by `charge(0)`.
    pub(crate) fn exhausted(&self) -> bool {
        self.0.failed.load(Ordering::Acquire)
    }
    fn check(&self) -> io::Result<()> {
        if self.exhausted() {
            Err(budget_error())
        } else {
            Ok(())
        }
    }
}
impl Default for WorkBudget {
    fn default() -> Self {
        Self::new(DEFAULT_WORK)
    }
}

enum Stage<R: Read> {
    Source {
        reader: R,
        budget: WorkBudget,
    },
    String {
        reader: Box<JsonStringReader<Window<R>>>,
        budget: WorkBudget,
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
        let (budget, count) = match self {
            Self::Source { reader, budget } => {
                budget.check()?;
                let amount = output
                    .len()
                    .min(budget.remaining().clamp(1, CHUNK as u64) as usize);
                let count = loop {
                    match reader.read(&mut output[..amount]) {
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => return Err(io::Error::other("native replay source read failed")),
                        Ok(count) => break count,
                    }
                };
                (budget, count)
            }
            Self::String { reader, budget } => {
                budget.check()?;
                let amount = output
                    .len()
                    .min(budget.remaining().clamp(1, CHUNK as u64) as usize);
                let count = reader.read(&mut output[..amount])?;
                (budget, count)
            }
        };
        // Actual source bytes AND output of every decoding layer are charged.
        // A failing batch is not delivered. Fixed per-layer buffers bound the
        // in-flight work before a failed charge; they are not whole strings.
        budget.charge(count as u64)?;
        Ok(count)
    }
}
impl<R: Read> Stage<R> {
    fn finish(self) -> io::Result<R> {
        match self {
            Self::Source { reader, budget } => {
                budget.check()?;
                Ok(reader)
            }
            Self::String { reader, budget } => {
                budget.check()?;
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
    budget: WorkBudget,
    eof: bool,
    failed: bool,
}
impl<R: Read> ReplayReader<R> {
    /// `reader` is already authorized and confined to first.start..first.end;
    /// its current offset is zero. No seek/path opening happens here.
    pub(crate) fn new(reader: R, plan: &DecodePlan, budget: WorkBudget) -> io::Result<Self> {
        budget.check()?;
        let mut stage = Stage::Source {
            reader,
            budget: budget.clone(),
        };
        for (index, range) in plan.ranges().iter().enumerate() {
            let window = Window {
                parent: Box::new(stage),
                skip: if index == 0 { 0 } else { range.start },
                remaining: range.end - range.start,
            };
            let reader = JsonStringReader::new(
                window,
                range.decoded_len,
                range.decoded_sha1,
                range.end - range.start,
            )?;
            stage = Stage::String {
                reader: Box::new(reader),
                budget: budget.clone(),
            };
        }
        Ok(Self {
            stage,
            budget,
            eof: false,
            failed: false,
        })
    }
    /// Final layer EOF is mandatory before finish. Finish then drains every
    /// parent tail, validates each parent digest, and returns the original R
    /// for its checked-handle completion. It never silently drains the target.
    pub(crate) fn finish(self) -> io::Result<R> {
        self.budget.check()?;
        if self.failed || !self.eof {
            return Err(incomplete());
        }
        let result = self.stage.finish();
        self.budget.check()?;
        result
    }
}
impl<R: Read> Read for ReplayReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.budget.check()?;
        if self.failed {
            return Err(incomplete());
        }
        if output.is_empty() {
            return Ok(0);
        }
        let result = self.stage.read(output);
        if result.is_err() || self.budget.exhausted() {
            self.failed = true;
        }
        // JsonStringReader intentionally sanitizes source errors, so recover
        // the shared budget classification at the public replay boundary.
        self.budget.check()?;
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
