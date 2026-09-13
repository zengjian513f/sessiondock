//! Bounded source capabilities, separate from decoded bytes. No snapshot or
//! checked file handle is retained by a descriptor, and no lock spans IO.
use super::*;
use crate::files::{FileError, ScopedFiles};

pub(super) struct Descriptor {
    pub(super) token: String,
    pub(super) source: Option<Arc<ImageSource>>,
    grant: Option<file_media::FileGrant>,
    pub(super) native_scope: Option<native_media::NativeScope>,
    pub(super) _charge: Charge,
}
struct DescriptorEntry {
    descriptor: Arc<Descriptor>,
    used: u64,
}
pub(super) struct Descriptors {
    entries: BTreeMap<String, DescriptorEntry>,
    clock: u64,
    maximum: usize,
}
impl Descriptors {
    pub(super) fn new(maximum: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            clock: 0,
            maximum,
        }
    }
    fn tick(&mut self) -> u64 {
        if self.clock == u64::MAX {
            let mut order: Vec<_> = self
                .entries
                .iter()
                .map(|(token, entry)| (entry.used, token.clone()))
                .collect();
            order.sort();
            for (index, (_, token)) in order.into_iter().enumerate() {
                self.entries
                    .get_mut(&token)
                    .expect("existing descriptor")
                    .used = index as u64;
            }
            self.clock = self.entries.len() as u64;
        }
        self.clock += 1;
        self.clock
    }
}
/// A held capability keeps only its private source alive and charged, even if
/// the lookup table evicts it while a worker is queued or a request is cancelled.
pub(crate) struct MediaTicket {
    pub(super) descriptor: Arc<Descriptor>,
}
impl MediaTicket {
    pub(crate) fn scope(&self) -> Option<(&str, &str)> {
        if let Some(scope) = &self.descriptor.native_scope {
            return Some((&scope.uid, &scope.agent));
        }
        self.descriptor
            .grant
            .as_ref()
            .map(file_media::FileGrant::scope)
    }
}
fn matches(descriptor: &Descriptor, image: &PreparedImage) -> bool {
    match (&descriptor.source, image.embedded_source()) {
        (Some(prior), Some(source)) => {
            descriptor.grant.is_none()
                && descriptor.native_scope == image.native_scope
                && Arc::ptr_eq(prior, &source)
        }
        (None, None) => descriptor.grant == image.grant,
        _ => false,
    }
}
fn error(error: MediaError) -> FileError {
    FileError::new(error.status(), "media_error", error.to_string())
}

impl MediaStore {
    /// Register selected image capabilities without reading or decoding bytes.
    /// A batch is protected from its own eviction and published atomically.
    pub(crate) fn register_prepared(
        &self,
        images: &[PreparedImage],
    ) -> Result<Vec<Value>, MediaError> {
        if images.is_empty() {
            return Ok(Vec::new());
        }
        let mut table = self
            .descriptors
            .lock()
            .map_err(|_| MediaError::Unavailable)?;
        let mut requested = BTreeMap::new();
        for image in images {
            if let Some(prior) = requested.insert(image.token.as_str(), image)
                && !image.same_source(prior)
            {
                return Err(MediaError::Unavailable);
            }
        }
        let mut missing = Vec::new();
        let mut required = 0usize;
        for (&token, image) in &requested {
            let source = image.embedded_source();
            let bytes = match source.as_deref().map(|source| &source.data) {
                Some(ImageData::Embedded { encoded, .. }) => encoded.capacity(),
                Some(ImageData::NativeSpan(span)) if image.native_scope.is_some() => {
                    let scope = image.native_scope.as_ref().expect("checked scope");
                    span.resident_len() + scope.uid.capacity() + scope.agent.capacity()
                }
                None => 0,
                _ => return Err(MediaError::Unsupported),
            };
            if let Some(entry) = table.entries.get(token) {
                if !matches(&entry.descriptor, image) {
                    return Err(MediaError::Unavailable);
                }
            } else {
                required = required.checked_add(bytes).ok_or(MediaError::Limit)?;
                missing.push((*image, source, bytes));
            }
        }
        while table.entries.len() + missing.len() > table.maximum
            || self
                .encoded_budget
                .used
                .load(Ordering::Acquire)
                .saturating_add(required)
                > self.encoded_budget.maximum
        {
            let candidate = table
                .entries
                .iter()
                .filter(|(token, _)| !requested.contains_key(token.as_str()))
                .min_by_key(|(_, entry)| entry.used)
                .map(|(token, _)| token.clone());
            let Some(candidate) = candidate else { break };
            table.entries.remove(&candidate);
        }
        for (image, source, bytes) in missing {
            self.encoded_budget.used.fetch_add(bytes, Ordering::AcqRel);
            let descriptor = Arc::new(Descriptor {
                token: image.token.clone(),
                source,
                grant: image.grant.clone(),
                native_scope: image.native_scope.clone(),
                _charge: Charge {
                    budget: self.encoded_budget.clone(),
                    bytes,
                },
            });
            table.entries.insert(
                image.token.clone(),
                DescriptorEntry {
                    descriptor,
                    used: 0,
                },
            );
        }
        for token in requested.keys() {
            let used = table.tick();
            table
                .entries
                .get_mut(*token)
                .expect("registered descriptor")
                .used = used;
        }
        Ok(images.iter().map(|image| json!({"src":format!("/api/media/{}",image.token),"alt":"会话图片","lazy":true})).collect())
    }

    pub(crate) fn ticket(&self, token: &str) -> Option<super::MediaTicket> {
        if token.len() != 32
            || !token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        let mut table = self.descriptors.lock().ok()?;
        let used = table.tick();
        let entry = table.entries.get_mut(token)?;
        entry.used = used;
        Some(MediaTicket {
            descriptor: entry.descriptor.clone(),
        })
    }

    /// Reauthorize first, including cache hits. A miss reserves bytes and a slot
    /// under the short cache lock, then validates using the retained new handle.
    /// Concurrent requests may decode the same token independently; cache
    /// retention must not become an input-admission rule.
    pub(crate) fn materialize(
        &self,
        ticket: MediaTicket,
        scope: Option<&ScopedFiles<'_>>,
    ) -> Result<Arc<MediaBlob>, FileError> {
        let descriptor = &ticket.descriptor;
        if descriptor.native_scope.is_some() {
            return Err(FileError::new(
                403,
                "media_native_scope",
                "原生图片需要当前来源授权读取器",
            ));
        }
        if !Arc::ptr_eq(&descriptor._charge.budget, &self.encoded_budget) {
            return Err(FileError::new(403, "media_scope", "图片不属于当前媒体服务"));
        }
        let prepared = if let Some(grant) = &descriptor.grant {
            let scope = scope
                .ok_or_else(|| FileError::new(403, "media_scope", "图片需要当前所选会话授权"))?;
            PreparedImage::authorized_file(
                descriptor.token.clone(),
                grant.clone(),
                grant.authorize(scope)?,
            )
        } else {
            let source = descriptor
                .source
                .as_ref()
                .ok_or_else(|| error(MediaError::Unavailable))?;
            PreparedImage::embedded(&NativeImage {
                source: source.clone(),
            })
            .map_err(error)?
        };
        let length = prepared.length().map_err(error)?;
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| error(MediaError::Unavailable))?;
        let used = tick(&mut cache);
        if let Some(entry) = cache.entries.get_mut(&descriptor.token) {
            // Tokens are random, but never use that as a substitute for source
            // consistency when a capability is already present.
            if entry.grant != descriptor.grant
                || match (&entry.source, &descriptor.source) {
                    (Some(prior), Some(source)) => !prior
                        .upgrade()
                        .is_some_and(|prior| Arc::ptr_eq(&prior, source)),
                    (None, None) => false,
                    _ => true,
                }
            {
                return Err(error(MediaError::Unavailable));
            }
            entry.used = used;
            return Ok(entry.blob.clone());
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
        let (bytes, mime, width, height) = prepared.read_checked(length).map_err(error)?;
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
                    source: prepared.source_weak(),
                    grant: descriptor.grant.clone(),
                    native_scope: None,
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
