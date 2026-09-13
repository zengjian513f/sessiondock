//! Private native-string descriptors. Paths are metadata, never open authority.
//! Every cold and warm read receives a currently authorized, JSON-unescaped
//! reader from the session layer. That layer must finish its checked file/range
//! verification before publishing the returned blob over HTTP.
use super::*;
use crate::files::FileError;
use crate::native_replay::DecodePlan;
use std::{
    io::{self, Read},
    path::PathBuf,
};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct NativeSpan {
    pub root: PathBuf,
    pub path: PathBuf,
    pub file_identity: String,
    pub record_start: u64,
    pub record_end: u64,
    pub start: u64,
    pub end: u64,
    /// None means one direct string; otherwise start/end describe the outer
    /// physical range and the immutable plan binds every nested string layer.
    pub plan: Option<DecodePlan>,
    /// Entire final JSON-unescaped string, including a data-URL header if present.
    pub decoded_len: u64,
    pub decoded_sha1: [u8; 20],
    pub encoded_offset: u64,
    pub payload_sha1: [u8; 20],
    pub mime: String,
}
impl NativeSpan {
    pub(super) fn resident_len(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.root.capacity()
            + self.path.capacity()
            + self.file_identity.capacity()
            + self.mime.capacity()
            // Self already includes the inline Option<DecodePlan> storage.
            + self.plan.as_ref().map_or(0, |plan| {
                plan.resident_len().saturating_sub(std::mem::size_of::<DecodePlan>())
            })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct NativeScope {
    pub uid: String,
    pub agent: String,
}

pub(super) fn semantic(mime: Mime, payload: &[u8; 20]) -> String {
    let mut hash = Sha1::new();
    hash.update(b"image-semantic-v2\0");
    hash.update(mime.text());
    hash.update([0]);
    hash.update(payload);
    format!("{:x}", hash.finalize())
}

impl NativeImage {
    pub(crate) fn from_native_span(mut span: NativeSpan) -> Result<Self, MediaError> {
        let mime = Mime::parse(&span.mime)?;
        if !span.root.is_absolute()
            || !span.path.is_absolute()
            || !span.path.starts_with(&span.root)
            || span.path == span.root
            || span.file_identity.is_empty()
            || span.record_start >= span.record_end
            || span.start <= span.record_start
            || span.end < span.start
            || span.end >= span.record_end
            || span.decoded_len > span.end - span.start
            || span.plan.as_ref().is_some_and(|plan| {
                plan.first().start != span.start
                    || plan.first().end != span.end
                    || plan.last().decoded_len != span.decoded_len
                    || plan.last().decoded_sha1 != span.decoded_sha1
            })
            || span.decoded_len <= span.encoded_offset
            || (span.encoded_offset == 0 && span.payload_sha1 != span.decoded_sha1)
        {
            return Err(MediaError::Invalid);
        }
        let encoded_len = span.decoded_len - span.encoded_offset;
        if encoded_len > MAX_ENCODED_BYTES as u64 {
            return Err(MediaError::Limit);
        }
        span.mime = mime.text().to_owned();
        Ok(Self {
            source: Arc::new(ImageSource {
                token: random_token()?,
                semantic: semantic(mime, &span.payload_sha1),
                data: ImageData::NativeSpan(span),
            }),
        })
    }
    pub(crate) fn native_span(&self) -> Option<&NativeSpan> {
        match &self.source.data {
            ImageData::NativeSpan(span) => Some(span),
            _ => None,
        }
    }
}

impl PreparedImage {
    pub(crate) fn native_span(
        image: &NativeImage,
        uid: &str,
        agent: &str,
    ) -> Result<Self, MediaError> {
        image.native_span().ok_or(MediaError::Unsupported)?;
        let (source, id) = uid.split_once(':').ok_or(MediaError::Invalid)?;
        if !matches!(source, "claude" | "codex" | "grok")
            || id.len() != 16
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(MediaError::Invalid);
        }
        // A shared inherited image can occur in several distinct selected views.
        // Derive stable per-scope tokens from its private random seed, without a
        // growing side table or exposing one view's token in another view.
        let mut hash = Sha1::new();
        hash.update(b"native-media-scope-v1\0");
        hash.update(image.source.token.as_bytes());
        hash.update([0]);
        hash.update(uid.as_bytes());
        hash.update([0]);
        hash.update(agent.as_bytes());
        let token = hash.finalize()[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(Self {
            token,
            grant: None,
            native_scope: Some(NativeScope {
                uid: uid.into(),
                agent: agent.into(),
            }),
            source: file_media::PreparedSource::Embedded(image.source.clone()),
        })
    }
}

impl MediaTicket {
    pub(crate) fn native_grant(&self) -> Option<(&str, &str, &NativeSpan)> {
        let scope = self.descriptor.native_scope.as_ref()?;
        let ImageData::NativeSpan(span) = &self.descriptor.source.as_ref()?.data else {
            return None;
        };
        Some((&scope.uid, &scope.agent, span))
    }
}

fn error(error: MediaError) -> FileError {
    FileError::new(error.status(), "media_error", error.to_string())
}
fn changed() -> FileError {
    FileError::new(
        409,
        "media_native_changed",
        "原生图片来源已变化，请重新加载会话",
    )
}

struct Verified<'a, R> {
    reader: R,
    span: &'a NativeSpan,
    count: u64,
    whole: Sha1,
    payload: Sha1,
    failed: bool,
}
impl<'a, R: Read> Verified<'a, R> {
    fn new(reader: R, span: &'a NativeSpan) -> Self {
        Self {
            reader,
            span,
            count: 0,
            whole: Sha1::new(),
            payload: Sha1::new(),
            failed: false,
        }
    }
    fn finish(mut self) -> Result<(), FileError> {
        let mut buffer = [0; 8192];
        loop {
            match self.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(changed()),
            }
        }
        if self.count != self.span.decoded_len
            || <[u8; 20]>::from(self.whole.finalize()) != self.span.decoded_sha1
            || <[u8; 20]>::from(self.payload.finalize()) != self.span.payload_sha1
        {
            return Err(changed());
        }
        Ok(())
    }
}
impl<R: Read> Read for Verified<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.failed {
            return Err(io::Error::other("native string reader failed"));
        }
        let limit = output
            .len()
            .min((self.span.decoded_len - self.count + 1) as usize);
        let count = match self.reader.read(&mut output[..limit]) {
            Ok(count) => count,
            Err(error) => {
                self.failed = error.kind() != io::ErrorKind::Interrupted;
                return Err(error);
            }
        };
        if count as u64 > self.span.decoded_len - self.count {
            self.failed = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native string length changed",
            ));
        }
        self.whole.update(&output[..count]);
        let skip = self
            .span
            .encoded_offset
            .saturating_sub(self.count)
            .min(count as u64) as usize;
        self.payload.update(&output[skip..count]);
        self.count += count as u64;
        Ok(count)
    }
}

impl MediaStore {
    /// Reader creation and final checked-range verification belong to sessions.
    /// This function never opens paths; even a warm hit consumes and hashes the
    /// supplied current source in full before returning a cached blob.
    pub(crate) fn materialize_native<R: Read>(
        &self,
        ticket: MediaTicket,
        reader: R,
    ) -> Result<Arc<MediaBlob>, FileError> {
        let descriptor = &ticket.descriptor;
        if !Arc::ptr_eq(&descriptor._charge.budget, &self.encoded_budget) {
            return Err(FileError::new(403, "media_scope", "图片不属于当前媒体服务"));
        }
        let (_, _, span) = ticket.native_grant().ok_or_else(|| {
            FileError::new(403, "media_native_scope", "原生图片需要当前来源授权读取器")
        })?;
        let mime = Mime::parse(&span.mime).map_err(error)?;
        let encoded = span.decoded_len - span.encoded_offset;
        let length = (encoded / 4 * 3).min(MAX_IMAGE_BYTES as u64) as usize;
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| error(MediaError::Unavailable))?;
        let used = tick(&mut cache);
        if let Some(entry) = cache.entries.get_mut(&descriptor.token) {
            if entry.native_scope != descriptor.native_scope
                || entry.grant.is_some()
                || !entry
                    .source
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .is_some_and(|prior| {
                        descriptor
                            .source
                            .as_ref()
                            .is_some_and(|source| Arc::ptr_eq(&prior, source))
                    })
            {
                return Err(error(MediaError::Unavailable));
            }
            entry.used = used;
            let blob = entry.blob.clone();
            drop(cache);
            Verified::new(reader, span).finish()?;
            return Ok(blob);
        }
        if length == 0 {
            return Err(error(MediaError::Limit));
        }
        while cache.entries.len() >= self.maximum_items
            || self
                .budget
                .used
                .load(Ordering::Acquire)
                .saturating_add(length)
                > self.budget.maximum
        {
            let victim = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(token, _)| token.clone());
            let Some(victim) = victim else { break };
            cache.entries.remove(&victim);
        }
        self.budget.used.fetch_add(length, Ordering::AcqRel);
        let charge = Charge {
            budget: self.budget.clone(),
            bytes: length,
        };
        drop(cache);
        let mut verified = Verified::new(reader, span);
        if span.encoded_offset != 0 {
            let mut header = vec![0; span.encoded_offset as usize];
            verified.read_exact(&mut header).map_err(|_| changed())?;
            let actual = std::str::from_utf8(&header)
                .ok()
                .and_then(|text| text.strip_prefix("data:")?.strip_suffix(";base64,"))
                .and_then(|text| text.split(';').next())
                .and_then(|text| Mime::parse(text).ok());
            if actual != Some(mime) {
                verified.finish()?;
                return Err(error(MediaError::Invalid));
            }
        }
        let mut encoded_bytes = Vec::new();
        let read = verified.read_to_end(&mut encoded_bytes);
        verified.finish()?;
        read.map_err(|_| error(MediaError::Invalid))?;
        let encoded_text =
            std::str::from_utf8(&encoded_bytes).map_err(|_| error(MediaError::Invalid))?;
        let decoded = decoded_length(encoded_text).map_err(error)?;
        let mut bytes = vec![0; length];
        let written = PYTHON_BASE64
            .decode_slice(python_base64_payload(encoded_text), &mut bytes)
            .map_err(|_| error(MediaError::Invalid))?;
        if written != decoded {
            return Err(error(MediaError::Invalid));
        }
        bytes.truncate(written);
        let (width, height) = inspect(mime, &bytes).unwrap_or((0, 0));
        let blob = Arc::new(MediaBlob {
            bytes,
            mime,
            width,
            height,
            _charge: charge,
        });
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| error(MediaError::Unavailable))?;
        let used = tick(&mut cache);
        if length <= self.budget.maximum && self.maximum_items > 0 {
            cache.entries.insert(
                descriptor.token.clone(),
                Entry {
                    source: descriptor.source.as_ref().map(Arc::downgrade),
                    grant: None,
                    native_scope: descriptor.native_scope.clone(),
                    blob: blob.clone(),
                    used,
                },
            );
        }
        drop(cache);
        Ok(blob)
    }
}
#[cfg(test)]
mod tests;
