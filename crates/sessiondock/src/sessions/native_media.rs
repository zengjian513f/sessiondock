//! Native span authority is the current full selected branch, not file_roots
//! and not the opaque token itself. Readers bind the published source stamp.
use super::records::string_reader::JsonStringReader;
use super::{Candidate, SessionError, SessionStore, native_input::CheckedNative};
use crate::media::NativeSpan;
use crate::native_replay::ReplayReader;
use std::io::{self, Read};

pub(crate) struct AuthorizedNativeReader {
    reader: NativeReader,
}
enum NativeReader {
    Direct(Box<JsonStringReader<CheckedNative>>),
    Nested(Box<ReplayReader<CheckedNative>>),
}
impl Read for AuthorizedNativeReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        match &mut self.reader {
            NativeReader::Direct(reader) => reader.read(output),
            NativeReader::Nested(reader) => reader.read(output),
        }
    }
}
impl AuthorizedNativeReader {
    pub(crate) fn finish(self) -> Result<(), SessionError> {
        let reader = match self.reader {
            NativeReader::Direct(reader) => reader
                .finish()
                .map_err(|_| SessionError::new(409, "原生图片内容或范围已变化，请重新加载会话"))?,
            NativeReader::Nested(reader) => reader
                .finish()
                .map_err(|_| SessionError::new(409, "原生嵌套图片的外层来源未通过校验"))?,
        };
        reader.finish()
    }
}

impl SessionStore {
    pub(crate) fn native_media_reader(
        &self,
        uid: &str,
        agent: &str,
        span: &NativeSpan,
    ) -> Result<AuthorizedNativeReader, SessionError> {
        // The same open history reads use: the index within its TTL, the
        // view re-stamped against its own file.
        let snapshot = self.snapshot(uid, agent)?;
        authorized_reader(snapshot.native_source(span)?, span)
    }
}

/// Open the span through the candidate stamp that produced the current
/// branch. Restamping after authorization and accepting a newer file would
/// reopen a race.
pub(super) fn authorized_reader(
    candidate: &Candidate,
    span: &NativeSpan,
) -> Result<AuthorizedNativeReader, SessionError> {
    {
        let expected = candidate
            .data_stamp()
            .ok_or_else(|| SessionError::new(409, "图片原生来源已消失"))?;
        if expected.file_identity != span.file_identity || span.record_end > expected.size {
            return Err(SessionError::new(409, "图片原生来源已替换，请重新加载会话"));
        }
        let reader = CheckedNative::open_range(
            &candidate.root,
            &candidate.data,
            expected,
            span.start,
            span.end,
        )?;
        let reader = if let Some(plan) = &span.plan {
            let replay = ReplayReader::new(reader, plan)
                .map_err(|_| SessionError::new(409, "原生嵌套图片范围无效"))?;
            NativeReader::Nested(Box::new(replay))
        } else {
            let direct = JsonStringReader::new(
                reader,
                span.decoded_len,
                span.decoded_digest,
                span.end - span.start,
            )
            .map_err(|_| SessionError::new(409, "原生图片区段无效"))?;
            NativeReader::Direct(Box::new(direct))
        };
        Ok(AuthorizedNativeReader { reader })
    }
}
