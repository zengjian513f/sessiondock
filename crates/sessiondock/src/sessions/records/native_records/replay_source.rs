//! Replays only reviewed tool strings from the stamped current native record.
//! Classifier callbacks never receive a path-opening capability of their own.
use super::*;
use crate::native_replay::{CheckedReplay, DecodePlan, ReplayReader, WorkBudget};
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
    budget: WorkBudget,
    failure: Failure,
}
fn classify(budget: &WorkBudget) -> SessionError {
    if budget.exhausted() {
        SessionError::new(413, "嵌套原生工具输出超过共享读取预算")
    } else {
        SessionError::new(409, "原生工具输出来源或解码范围已变化，请重试")
    }
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
                let error = classify(&self.budget);
                retain(&self.failure, error.clone());
                Err(io::Error::other(error))
            }
        }
    }
}
impl CheckedReplay for NativeToolReader {
    fn finish(self: Box<Self>) -> Result<(), String> {
        let Self {
            reader,
            budget,
            failure,
        } = *self;
        // Draining parent tails happens inside finish, so the budget can only
        // be classified after that failure, never from the pre-finish state.
        let result = reader
            .finish()
            .map_err(|_| classify(&budget))
            .and_then(CheckedNative::finish);
        result.map_err(|error| retain(&failure, error))
    }
}

/// Read one ordinary giant string back from the stamped current record
/// through the same checked range reader images use, verifying the span's
/// decoded length and SHA-1 before the text becomes part of the row. The
/// record's ordinary-body budget (`LINE_LIMIT`) is shared by all its spans.
fn materialize_text(
    candidate: &Candidate,
    start: u64,
    end: u64,
    span: &TextSpan,
    remaining: &mut usize,
    failure: &Failure,
) -> Result<String, String> {
    if let Some(error) = failure.borrow().as_ref() {
        return Err(error.message.clone());
    }
    let decoded = usize::try_from(span.decoded_len()).map_err(|_| "原生文本区段溢出")?;
    if decoded > *remaining {
        return Err("普通原生记录超过 64 MiB 上限".into());
    }
    *remaining -= decoded;
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
    let mut remaining = LINE_LIMIT;
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
                decoded_sha1: *image.span.digest(),
                mime: image.mime,
                encoded_offset: image.encoded_offset,
                payload_sha1: image.payload_sha1,
                plan,
            })
            .map_err(|error| error.to_string())
        },
        |plan: &DecodePlan, budget: WorkBudget| {
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
                let reader = ReplayReader::new(checked, &plan, budget.clone()).map_err(|_| {
                    SessionError::new(
                        if budget.exhausted() { 413 } else { 409 },
                        "原生工具输出解码无法建立",
                    )
                })?;
                Ok(Box::new(NativeToolReader {
                    reader,
                    budget,
                    failure: failure.clone(),
                }))
            };
            open().map_err(|error| retain(&failure, error))
        },
        |span| materialize_text(candidate, start, end, span, &mut remaining, &failure),
    );
    if let Some(error) = failure.borrow_mut().take() {
        return Err(error);
    }
    // Replayed envelopes can contain no image at all (for example a large
    // protocol prefix followed by ordinary output). Enforce the residual body
    // budget even when no image/error sidecar remains after transformation.
    Ok(result.and_then(|(row, sidecars)| {
        if ordinary_body_fits(&row) {
            Ok((row, sidecars))
        } else {
            Err("普通原生记录超过 64 MiB 上限".into())
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_replay::StringRange;
    use sha1::{Digest, Sha1};

    #[test]
    fn finish_classifies_parent_tail_failure_after_reading_it() {
        for (budget_limit, wrong_hash, prior_status, expected) in [
            (32_768, false, None, 413),
            (1_000_000, true, None, 409),
            (32_768, false, Some(503), 503),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let path = root.join("synthetic.jsonl");
            let parent = format!("\"x\"{}", "T".repeat(65_536));
            let physical = serde_json::to_vec(&parent).unwrap();
            std::fs::write(&path, &physical).unwrap();
            let stamp = crate::sessions::stamp(&path).unwrap();
            let mut parent_hash: [u8; 20] = Sha1::digest(parent.as_bytes()).into();
            if wrong_hash {
                parent_hash[0] ^= 1;
            }
            let plan = DecodePlan::new(vec![
                StringRange {
                    start: 1,
                    end: physical.len() as u64 - 1,
                    decoded_len: parent.len() as u64,
                    decoded_sha1: parent_hash,
                },
                StringRange {
                    start: 1,
                    end: 2,
                    decoded_len: 1,
                    decoded_sha1: Sha1::digest(b"x").into(),
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
            let budget = WorkBudget::new(budget_limit);
            let failure: Failure = Rc::new(RefCell::new(None));
            let mut reader = NativeToolReader {
                reader: ReplayReader::new(checked, &plan, budget.clone()).unwrap(),
                budget: budget.clone(),
                failure: failure.clone(),
            };
            let mut image = Vec::new();
            reader.read_to_end(&mut image).unwrap();
            assert_eq!(image, b"x");
            assert!(
                !budget.exhausted(),
                "only parent finish may exhaust the budget"
            );
            if let Some(status) = prior_status {
                retain(&failure, SessionError::new(status, "first failure"));
            }
            assert!(Box::new(reader).finish().is_err());
            assert_eq!(failure.borrow().as_ref().unwrap().status, expected);
        }
    }
}
