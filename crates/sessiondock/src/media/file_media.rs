//! File capabilities are revalidated against the current selected native view.
use super::*;
use crate::files::{CheckedImage, FileError, FileVersion, ScopedFiles};
use std::{cell::RefCell, io::Read};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct FileGrant {
    uid: String,
    agent: Option<String>,
    reference: String,
    version: FileVersion,
}

#[cfg(test)]
pub(crate) struct FileTicket {
    grant: FileGrant,
    blob: Arc<MediaBlob>,
}
#[cfg(test)]
impl FileTicket {
    pub(super) fn new(grant: FileGrant, blob: Arc<MediaBlob>) -> Self {
        Self { grant, blob }
    }
    pub(crate) fn authorize(self, scope: &ScopedFiles<'_>) -> Result<Arc<MediaBlob>, FileError> {
        if scope.identity() != (self.grant.uid.as_str(), self.grant.agent.as_deref()) {
            return Err(FileError::new(403, "media_scope", "图片不属于当前所选会话"));
        }
        let current = scope.image(&self.grant.reference)?;
        if current.version() != &self.grant.version {
            return Err(FileError::new(
                409,
                "media_changed",
                "图片文件已变化，请重新加载会话",
            ));
        }
        current.verify()?;
        Ok(self.blob)
    }
}

impl FileGrant {
    pub(super) fn scope(&self) -> (&str, &str) {
        (&self.uid, self.agent.as_deref().unwrap_or(""))
    }
    pub(super) fn authorize(&self, scope: &ScopedFiles<'_>) -> Result<CheckedImage, FileError> {
        if scope.identity() != (self.uid.as_str(), self.agent.as_deref()) {
            return Err(FileError::new(403, "media_scope", "图片不属于当前所选会话"));
        }
        let current = scope.image(&self.reference)?;
        if current.version() != &self.version {
            return Err(FileError::new(
                409,
                "media_changed",
                "图片文件已变化，请重新加载会话",
            ));
        }
        current.verify()?;
        Ok(current)
    }
}

pub(super) enum PreparedSource {
    Embedded(Arc<ImageSource>),
    File(Box<RefCell<CheckedImage>>),
}
pub(crate) struct PreparedImage {
    pub(super) token: String,
    pub(super) grant: Option<FileGrant>,
    pub(super) native_scope: Option<native_media::NativeScope>,
    pub(super) source: PreparedSource,
}
impl PreparedImage {
    pub(crate) fn is_file(&self) -> bool {
        self.grant.is_some()
    }
    pub(crate) fn embedded(image: &NativeImage) -> Result<Self, MediaError> {
        if image.file_ref().is_some() || image.native_span().is_some() {
            return Err(MediaError::Unsupported);
        }
        Ok(Self {
            token: image.source.token.clone(),
            grant: None,
            native_scope: None,
            source: PreparedSource::Embedded(image.source.clone()),
        })
    }
    pub(crate) fn file(scope: &ScopedFiles<'_>, reference: &str) -> Result<Self, FileError> {
        let reference = crate::files::normalize_media_ref(reference)?;
        let checked = scope.image(&reference)?;
        let (uid, agent) = scope.identity();
        let grant = FileGrant {
            uid: uid.into(),
            agent: agent.map(str::to_owned),
            reference,
            version: checked.version().clone(),
        };
        let token = random_token()
            .map_err(|_| FileError::new(503, "media_unavailable", "图片服务暂不可用"))?;
        Ok(Self {
            token,
            grant: Some(grant),
            native_scope: None,
            source: PreparedSource::File(Box::new(RefCell::new(checked))),
        })
    }
    pub(super) fn same_source(&self, other: &Self) -> bool {
        match (&self.source, &other.source) {
            (PreparedSource::Embedded(a), PreparedSource::Embedded(b)) => {
                Arc::ptr_eq(a, b) && self.native_scope == other.native_scope
            }
            (PreparedSource::File(_), PreparedSource::File(_)) => self.grant == other.grant,
            _ => false,
        }
    }
    pub(super) fn embedded_source(&self) -> Option<Arc<ImageSource>> {
        match &self.source {
            PreparedSource::Embedded(source) => Some(source.clone()),
            PreparedSource::File(_) => None,
        }
    }
    pub(super) fn authorized_file(token: String, grant: FileGrant, checked: CheckedImage) -> Self {
        Self {
            token,
            grant: Some(grant),
            native_scope: None,
            source: PreparedSource::File(Box::new(RefCell::new(checked))),
        }
    }
    pub(crate) fn length(&self) -> Result<usize, MediaError> {
        match &self.source {
            PreparedSource::Embedded(source) => match &source.data {
                ImageData::Embedded { encoded, .. } => decoded_length(encoded),
                _ => Err(MediaError::Unavailable),
            },
            PreparedSource::File(checked) => {
                usize::try_from(checked.borrow().size()).map_err(|_| MediaError::Limit)
            }
        }
    }
    #[cfg(test)]
    pub(super) fn matches_entry(&self, entry: &Entry) -> bool {
        match &self.source {
            PreparedSource::Embedded(source) => {
                entry.grant.is_none()
                    && entry.native_scope == self.native_scope
                    && entry
                        .source
                        .as_ref()
                        .and_then(Weak::upgrade)
                        .is_some_and(|prior| Arc::ptr_eq(source, &prior))
            }
            PreparedSource::File(_) => self.grant == entry.grant,
        }
    }
    pub(super) fn source_weak(&self) -> Option<Weak<ImageSource>> {
        match &self.source {
            PreparedSource::Embedded(source) => Some(Arc::downgrade(source)),
            _ => None,
        }
    }
    pub(super) fn read_checked(
        &self,
        length: usize,
    ) -> Result<(Vec<u8>, Mime, u32, u32), MediaError> {
        let (bytes, mime) = match &self.source {
            PreparedSource::Embedded(source) => {
                let ImageData::Embedded { mime, encoded } = &source.data else {
                    return Err(MediaError::Unavailable);
                };
                // Reserve the exact decoded allocation; decode_vec may reserve a
                // padded upper bound beyond the shared cache accounting.
                let mut bytes = vec![0; length];
                let written = PYTHON_BASE64
                    .decode_slice(python_base64_payload(encoded), &mut bytes)
                    .map_err(|_| MediaError::Invalid)?;
                if written != length {
                    return Err(MediaError::Invalid);
                }
                (bytes, *mime)
            }
            PreparedSource::File(checked) => {
                let mut checked = checked.borrow_mut();
                checked.verify().map_err(|_| MediaError::Invalid)?;
                let mut bytes = vec![0; length];
                checked
                    .read_exact(&mut bytes)
                    .map_err(|_| MediaError::Invalid)?;
                checked.verify().map_err(|_| MediaError::Invalid)?;
                if checked.remaining() != 0 {
                    return Err(MediaError::Invalid);
                }
                let mime = sniff(&bytes)
                    .or_else(|| checked.mime_hint().and_then(|hint| Mime::parse(hint).ok()))
                    .ok_or(MediaError::Unsupported)?;
                (bytes, mime)
            }
        };
        let (width, height) = inspect(mime, &bytes).unwrap_or((0, 0));
        Ok((bytes, mime, width, height))
    }
}
fn sniff(bytes: &[u8]) -> Option<Mime> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Mime::Png)
    } else if bytes.starts_with(b"\xff\xd8") {
        Some(Mime::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Mime::Gif)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(Mime::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(Mime::Bmp)
    } else if bytes.get(4..8) == Some(b"ftyp") {
        Some(Mime::Avif)
    } else {
        None
    }
}
pub(crate) fn failure(status: u16, code: &str, message: &str) -> Value {
    json!({"alt":"会话图片","error":{"status":status,"code":code,"message":message}})
}

/// A text-discovered reference that Python's `media.register_path` would also
/// register nothing for (missing file, no cwd for a relative path, not a
/// regular file, over the shared 32 MiB limit, unparsable path): the message
/// keeps its text and no placeholder is projected. Other failures stay visible
/// so the operator can act on the reason.
pub(crate) fn silent_failure(error: &FileError) -> bool {
    matches!(
        error.code,
        "file_not_found"
            | "file_not_referenced"
            | "file_cwd_unavailable"
            | "file_image_required"
            | "file_special_forbidden"
            | "file_image_budget"
            | "file_path_depth"
            | "file_path_invalid"
            | "file_foreign_path"
            | "file_path_encoding"
            | "file_absolute_path_required"
    )
}

#[cfg(test)]
mod tests;
