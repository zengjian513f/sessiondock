//! Replays only reviewed tool strings from the stamped current native record.
//! Classifier callbacks never receive a path-opening capability of their own.
use super::*;
use crate::native_replay::{CheckedReplay, DecodePlan, ReplayReader};
use crate::sessions::native_input::CheckedNative;
use crate::sessions::records::{scanner::TextSpan, string_reader::JsonStringReader};
use std::{cell::RefCell, rc::Rc};

type Failure = Rc<RefCell<Option<SessionError>>>;

fn retain(failure: &Failure, error: SessionError) -> String {
    let mut first = failure.borrow_mut();
    first.get_or_insert(error).message.clone()
}

struct NativeToolReader {
    reader: ReplayReader<CheckedNative>,
    failure: Failure,
}
fn replay_changed() -> SessionError {
    SessionError::new(409, "原生工具输出来源或解码范围已变化，请重试")
}
impl Read for NativeToolReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if let Some(error) = self.failure.borrow().as_ref() {
            return Err(io::Error::other(error.clone()));
        }
        match self.reader.read(bytes) {
            Ok(count) => Ok(count),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Err(error),
            Err(_) => {
                let error = replay_changed();
                retain(&self.failure, error.clone());
                Err(io::Error::other(error))
            }
        }
    }
}
impl CheckedReplay for NativeToolReader {
    fn finish(self: Box<Self>) -> Result<(), String> {
        let Self { reader, failure } = *self;
        let result = reader
            .finish()
            .map_err(|_| replay_changed())
            .and_then(CheckedNative::finish);
        result.map_err(|error| retain(&failure, error))
    }
}

/// Read one ordinary giant string back from the stamped current record
/// through the same checked range reader images use, verifying the span's
/// decoded length and fingerprint before the text becomes part of the row. The
/// source range bounds the read; there is no ordinary-body size quota.
fn materialize_text(
    candidate: &Candidate,
    start: u64,
    end: u64,
    span: &TextSpan,
    failure: &Failure,
) -> Result<String, String> {
    if let Some(error) = failure.borrow().as_ref() {
        return Err(error.message.clone());
    }
    let decoded = usize::try_from(span.decoded_len()).map_err(|_| "原生文本区段溢出")?;
    let read = || -> Result<String, SessionError> {
        let stamp = candidate
            .data_stamp()
            .ok_or_else(|| SessionError::new(409, "原生文本来源已消失"))?;
        let (physical_start, physical_end) = (start + span.start(), start + span.end());
        if span.start() == 0 || physical_end >= end {
            return Err(SessionError::new(403, "文本读回范围不属于当前原生记录"));
        }
        let checked = CheckedNative::open_range(
            &candidate.root,
            &candidate.data,
            stamp,
            physical_start,
            physical_end,
        )?;
        let mut reader = JsonStringReader::new(
            checked,
            span.decoded_len(),
            *span.digest(),
            physical_end - physical_start,
        )
        .map_err(|_| SessionError::new(409, "原生文本区段无效"))?;
        let mut text = String::new();
        text.try_reserve_exact(decoded)
            .map_err(|_| SessionError::new(503, "原生文本缓冲区分配失败"))?;
        // The physical range bounds the read; EOF is where the reader verifies
        // the decoded length and digest, and `finish` refuses anything less.
        reader
            .read_to_string(&mut text)
            .map_err(|_| SessionError::new(503, "原生文本在读取期间变化，请重试"))?;
        if text.len() != decoded {
            return Err(SessionError::new(503, "原生文本在读取期间变化，请重试"));
        }
        reader
            .finish()
            .map_err(|_| SessionError::new(503, "原生文本在读取期间变化，请重试"))?
            .finish()?;
        Ok(text)
    };
    read().map_err(|error| retain(failure, error))
}

pub(super) fn prepare(
    candidate: &Candidate,
    start: u64,
    end: u64,
    root: scanner::Node,
) -> Result<Result<(Value, Vec<native_images::Sidecar>), String>, SessionError> {
    let failure: Failure = Rc::new(RefCell::new(None));
    let result = native_images::prepare_with_replay(
        candidate.source,
        root,
        |image| {
            let stamp = candidate.data_stamp().ok_or("图片原生来源缺失")?;
            let plan = image
                .plan
                .map(|plan| plan.with_outer_offset(start))
                .transpose()
                .map_err(|_| "原生图片嵌套范围无效")?;
            let physical_start = plan
                .as_ref()
                .map_or(start + image.span.start(), |plan| plan.first().start);
            let physical_end = plan
                .as_ref()
                .map_or(start + image.span.end(), |plan| plan.first().end);
            NativeImage::from_native_span(NativeSpan {
                root: candidate.root.clone(),
                path: candidate.data.clone(),
                file_identity: stamp.file_identity.clone(),
                record_start: start,
                record_end: end,
                start: physical_start,
                end: physical_end,
                decoded_len: image.span.decoded_len(),
                decoded_digest: *image.span.digest(),
                mime: image.mime,
                encoded_offset: image.encoded_offset,
                payload_digest: image.payload_digest,
                plan,
            })
            .map_err(|error| error.to_string())
        },
        |plan: &DecodePlan| {
            if let Some(error) = failure.borrow().as_ref() {
                return Err(error.message.clone());
            }
            let open = || -> Result<Box<dyn CheckedReplay>, SessionError> {
                let outer = plan.first();
                if outer.start == 0 || outer.end >= end - start {
                    return Err(SessionError::new(403, "工具解码范围不属于当前原生记录"));
                }
                let plan = plan
                    .with_outer_offset(start)
                    .map_err(|_| SessionError::new(403, "工具解码范围无效"))?;
                let stamp = candidate
                    .data_stamp()
                    .ok_or_else(|| SessionError::new(409, "原生工具来源已消失"))?;
                let checked = CheckedNative::open_range(
                    &candidate.root,
                    &candidate.data,
                    stamp,
                    plan.first().start,
                    plan.first().end,
                )?;
                let reader = ReplayReader::new(checked, &plan)
                    .map_err(|_| SessionError::new(409, "原生工具输出解码无法建立"))?;
                Ok(Box::new(NativeToolReader {
                    reader,
                    failure: failure.clone(),
                }))
            };
            open().map_err(|error| retain(&failure, error))
        },
        |span| materialize_text(candidate, start, end, span, &failure),
    );
    if let Some(error) = failure.borrow_mut().take() {
        return Err(error);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::{Digest, Fingerprint};
    use crate::native_replay::StringRange;

    #[test]
    fn finish_retains_parent_tail_failure_and_preserves_the_first_error() {
        for (prior_status, expected) in [(None, 409), (Some(503), 503)] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let path = root.join("synthetic.jsonl");
            let parent = format!("\"x\"{}", "T".repeat(65_536));
            let physical = serde_json::to_vec(&parent).unwrap();
            std::fs::write(&path, &physical).unwrap();
            let stamp = crate::sessions::stamp(&path).unwrap();
            let mut parent_hash: Digest = Fingerprint::digest(parent.as_bytes());
            parent_hash[0] ^= 1;
            let plan = DecodePlan::new(vec![
                StringRange {
                    start: 1,
                    end: physical.len() as u64 - 1,
                    decoded_len: parent.len() as u64,
                    decoded_digest: parent_hash,
                },
                StringRange {
                    start: 1,
                    end: 2,
                    decoded_len: 1,
                    decoded_digest: Fingerprint::digest(b"x"),
                },
            ])
            .unwrap();
            let checked = CheckedNative::open_range(
                &root,
                &path,
                &stamp,
                plan.first().start,
                plan.first().end,
            )
            .unwrap();
            let failure: Failure = Rc::new(RefCell::new(None));
            let mut reader = NativeToolReader {
                reader: ReplayReader::new(checked, &plan).unwrap(),
                failure: failure.clone(),
            };
            let mut image = Vec::new();
            reader.read_to_end(&mut image).unwrap();
            assert_eq!(image, b"x");
            if let Some(status) = prior_status {
                retain(&failure, SessionError::new(status, "first failure"));
            }
            assert!(Box::new(reader).finish().is_err());
            assert_eq!(failure.borrow().as_ref().unwrap().status, expected);
        }
    }
}
