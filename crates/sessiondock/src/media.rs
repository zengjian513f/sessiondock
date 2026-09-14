//! Private image projection. File capabilities require an explicit selected scope;
//! no remote fetch or public raw native payloads.

mod descriptors;
mod discovery;
mod file_media;
mod formats;
mod native_media;
pub(crate) use descriptors::MediaTicket;
pub(crate) use discovery::discover;
#[cfg(test)]
pub(crate) use file_media::FileTicket;
pub(crate) use file_media::PreparedImage;
pub(crate) use file_media::{failure, silent_failure};
pub(crate) use native_media::NativeSpan;

#[cfg(test)]
use base64::engine::general_purpose::STANDARD;
use base64::{
    Engine as _, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
#[cfg(test)]
use std::collections::BTreeSet;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};

/// One decoded image may be at most 32 MiB.
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ENCODED_BYTES: usize = MAX_IMAGE_BYTES * 4 / 3 + 16;
pub const MAX_CACHE_BYTES: usize = 128 * 1024 * 1024;
const CACHE_ITEMS: usize = 512;
static PYTHON_BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::RequireCanonical)
        .with_decode_allow_trailing_bits(true),
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaError {
    Unsupported,
    Invalid,
    Limit,
    Unavailable,
}
impl MediaError {
    pub fn status(self) -> u16 {
        match self {
            Self::Unsupported => 501,
            Self::Invalid => 422,
            Self::Limit => 413,
            Self::Unavailable => 503,
        }
    }
}
impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "此媒体格式或编码特性尚未支持；外链不会自动读取",
            Self::Invalid => "媒体图片的编码、声明格式或文件结构无效",
            Self::Limit => "图片超过 32 MiB 限制",
            Self::Unavailable => "媒体服务暂不可用",
        })
    }
}
impl std::error::Error for MediaError {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mime {
    Png,
    Jpeg,
    Gif,
    Webp,
    Avif,
    Bmp,
}
impl Mime {
    fn parse(value: &str) -> Result<Self, MediaError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "image/png" | "image/apng" => Ok(Self::Png),
            "image/jpeg" | "image/jpg" => Ok(Self::Jpeg),
            "image/gif" => Ok(Self::Gif),
            "image/webp" => Ok(Self::Webp),
            "image/avif" => Ok(Self::Avif),
            "image/bmp" | "image/x-ms-bmp" => Ok(Self::Bmp),
            _ => Err(MediaError::Unsupported),
        }
    }
    fn text(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            Self::Bmp => "image/bmp",
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Avif => "avif",
            Self::Bmp => "bmp",
        }
    }
}
struct ImageSource {
    token: String,
    semantic: String,
    data: ImageData,
}
enum ImageData {
    Embedded { mime: Mime, encoded: String },
    FileReference(String),
    NativeSpan(NativeSpan),
}
/// Pure private native data, never serialized/debugged into message JSON. A
/// clone shares its stable random token; semantic_key is independent of tokens.
#[derive(Clone)]
pub struct NativeImage {
    source: Arc<ImageSource>,
}
impl NativeImage {
    pub fn from_block(block: &Value) -> Result<Option<Self>, String> {
        Ok(Self::parse(block).ok().flatten())
    }
    fn parse(block: &Value) -> Result<Option<Self>, MediaError> {
        for object in [block.get("source"), block.get("file"), Some(block)]
            .into_iter()
            .flatten()
            .filter(|object| object.is_object())
        {
            let first_text = |keys: &[&str]| {
                keys.iter()
                    .find_map(|key| object[*key].as_str().filter(|value| !value.is_empty()))
            };
            if let (Some(data), Some(mime)) = (
                first_text(&["data", "base64"]),
                first_text(&["media_type", "mime_type", "mimeType", "type"])
                    .and_then(|value| Mime::parse(value).ok()),
            ) && decoded_length(data).is_ok()
            {
                return Self::embedded(mime, data).map(Some);
            }
            let url = ["url", "image_url"].iter().find_map(|key| {
                object[*key]
                    .as_str()
                    .or_else(|| object[*key]["url"].as_str())
                    .filter(|value| !value.is_empty())
            });
            if let Some(url) = url {
                if remote_reference(url) {
                    return Self::file_reference(url.to_owned()).map(Some);
                }
                if let Some((head, data)) = url
                    .strip_prefix("data:")
                    .and_then(|url| url.split_once(','))
                {
                    if head.contains(";base64")
                        && let Some(mime) = head
                            .split(';')
                            .next()
                            .and_then(|value| Mime::parse(value).ok())
                        && decoded_length(data).is_ok()
                    {
                        return Self::embedded(mime, data).map(Some);
                    }
                } else if let Ok(reference) = crate::files::normalize_media_ref(url) {
                    return Self::file_reference(reference).map(Some);
                }
            }
            if let Some(path) = object["path"].as_str()
                && let Ok(reference) = crate::files::normalize_media_ref(path)
            {
                return Self::file_reference(reference).map(Some);
            }
        }
        Ok(None)
    }
    fn embedded(mime: Mime, encoded: &str) -> Result<Self, MediaError> {
        Ok(Self {
            source: Arc::new(ImageSource {
                token: random_token()?,
                semantic: native_media::semantic(mime, &Sha1::digest(encoded).into()),
                data: ImageData::Embedded {
                    mime,
                    encoded: encoded.into(),
                },
            }),
        })
    }
    pub fn encoded_len(&self) -> usize {
        match &self.source.data {
            ImageData::Embedded { encoded, .. } => encoded.len(),
            ImageData::FileReference(reference) => reference.len(),
            ImageData::NativeSpan(span) => (span.decoded_len - span.encoded_offset) as usize,
        }
    }
    /// Resident source metadata, distinct from logical base64 size for spans.
    pub fn resident_len(&self) -> usize {
        match &self.source.data {
            ImageData::Embedded { encoded, .. } => encoded.capacity(),
            ImageData::FileReference(reference) => reference.capacity(),
            ImageData::NativeSpan(span) => span.resident_len(),
        }
    }
    pub fn semantic_key(&self) -> &str {
        &self.source.semantic
    }
    pub(crate) fn file_ref(&self) -> Option<&str> {
        match &self.source.data {
            ImageData::FileReference(reference) if !remote_reference(reference) => Some(reference),
            _ => None,
        }
    }
    pub(crate) fn remote_ref(&self) -> Option<&str> {
        match &self.source.data {
            ImageData::FileReference(reference) if remote_reference(reference) => Some(reference),
            _ => None,
        }
    }
    fn file_reference(reference: String) -> Result<Self, MediaError> {
        let mut digest = Sha1::new();
        digest.update(b"native-file-reference\0");
        digest.update(reference.as_bytes());
        Ok(Self {
            source: Arc::new(ImageSource {
                token: random_token()?,
                semantic: format!("{:x}", digest.finalize()),
                data: ImageData::FileReference(reference),
            }),
        })
    }
}

/// Python media._remote: preserve an HTTP(S) URL for the browser to load.
pub(crate) fn remote_reference(reference: &str) -> bool {
    let Some((scheme, rest)) = reference.split_once("://") else {
        return false;
    };
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !rest.split(['/', '?', '#']).next().unwrap_or("").is_empty()
}

pub(crate) fn remote_image(reference: &str) -> Value {
    json!({"src": reference, "mime": "", "alt": "会话图片", "external": true})
}
fn random_token() -> Result<String, MediaError> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| MediaError::Unavailable)?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn decoded_length(encoded: &str) -> Result<usize, MediaError> {
    if encoded.len() > MAX_ENCODED_BYTES {
        return Err(MediaError::Limit);
    }
    let encoded = python_base64_payload(encoded);
    if encoded.is_empty() || !encoded.len().is_multiple_of(4) {
        return Err(MediaError::Invalid);
    }
    let padding = encoded
        .as_bytes()
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'=')
        .count();
    if padding > 2 {
        return Err(MediaError::Invalid);
    }
    let length = encoded.len() / 4 * 3 - padding;
    if length == 0 || length > MAX_IMAGE_BYTES {
        return Err(MediaError::Limit);
    }
    Ok(length)
}

/// Python accepts trailing padding after a complete four-character group.
/// Partial groups still require canonical padding; the decoder validates them.
fn python_base64_payload(encoded: &str) -> &str {
    let unpadded = encoded.trim_end_matches('=');
    if unpadded.len().is_multiple_of(4) {
        unpadded
    } else {
        encoded
    }
}

struct Budget {
    used: AtomicUsize,
    maximum: usize,
}
struct Charge {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
pub struct MediaBlob {
    bytes: Vec<u8>,
    mime: Mime,
    #[cfg_attr(not(test), allow(dead_code))]
    width: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    height: u32,
    _charge: Charge,
}
impl MediaBlob {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn mime(&self) -> &str {
        self.mime.text()
    }
    pub fn extension(&self) -> &str {
        self.mime.extension()
    }
}
struct Entry {
    source: Option<Weak<ImageSource>>,
    grant: Option<file_media::FileGrant>,
    native_scope: Option<native_media::NativeScope>,
    blob: Arc<MediaBlob>,
    used: u64,
}
struct Cache {
    entries: BTreeMap<String, Entry>,
    clock: u64,
}
pub struct MediaStore {
    descriptors: Mutex<descriptors::Descriptors>,
    encoded_budget: Arc<Budget>,
    cache: Mutex<Cache>,
    budget: Arc<Budget>,
    maximum_items: usize,
}
impl Default for MediaStore {
    fn default() -> Self {
        Self::new()
    }
}
impl MediaStore {
    pub fn new() -> Self {
        Self::with_limits(CACHE_ITEMS, MAX_CACHE_BYTES)
    }
    fn with_limits(maximum_items: usize, bytes: usize) -> Self {
        Self {
            descriptors: Mutex::new(descriptors::Descriptors::new(1024)),
            encoded_budget: Arc::new(Budget {
                used: AtomicUsize::new(0),
                maximum: MAX_CACHE_BYTES,
            }),
            cache: Mutex::new(Cache {
                entries: BTreeMap::new(),
                clock: 0,
            }),
            budget: Arc::new(Budget {
                used: AtomicUsize::new(0),
                maximum: bytes,
            }),
            maximum_items,
        }
    }
    /// Run off the reactor. The whole batch is protected from its own LRU
    /// eviction; failed batches publish no partially registered new tokens.
    #[cfg(test)]
    pub fn project(&self, images: &[NativeImage]) -> Result<Vec<Value>, MediaError> {
        let mut output = vec![Value::Null; images.len()];
        let mut prepared = Vec::new();
        let mut positions = Vec::new();
        for (position, image) in images.iter().enumerate() {
            if let Some(reference) = image.remote_ref() {
                output[position] = remote_image(reference);
            } else {
                prepared.push(PreparedImage::embedded(image)?);
                positions.push(position);
            }
        }
        for (position, value) in positions.into_iter().zip(self.project_prepared(&prepared)?) {
            output[position] = value;
        }
        Ok(output)
    }
    #[cfg(test)]
    pub(crate) fn project_prepared(
        &self,
        images: &[PreparedImage],
    ) -> Result<Vec<Value>, MediaError> {
        if images.is_empty() {
            return Ok(Vec::new());
        }
        let mut cache = self.cache.lock().map_err(|_| MediaError::Unavailable)?;
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
            let length = image.length()?;
            if let Some(entry) = cache.entries.get(token) {
                if !image.matches_entry(entry) {
                    return Err(MediaError::Unavailable);
                }
            } else {
                required = required.checked_add(length).ok_or(MediaError::Limit)?;
                missing.push((*image, length));
            }
        }
        let protected: BTreeSet<_> = requested.keys().copied().collect();
        while cache.entries.len() + missing.len() > self.maximum_items
            || self
                .budget
                .used
                .load(Ordering::Acquire)
                .saturating_add(required)
                > self.budget.maximum
        {
            let candidate = cache
                .entries
                .iter()
                .filter(|(token, _)| !protected.contains(token.as_str()))
                .min_by_key(|(_, entry)| entry.used)
                .map(|(token, _)| token.clone());
            let Some(candidate) = candidate else { break };
            cache.entries.remove(&candidate);
        }
        // Only project increments the budget, under this lock. Blob destruction
        // may decrement it concurrently, including after an entry was evicted.
        self.budget.used.fetch_add(required, Ordering::AcqRel);
        let mut reserved = Charge {
            budget: self.budget.clone(),
            bytes: required,
        };
        let mut staged = Vec::with_capacity(missing.len());
        let mut failures = BTreeMap::new();
        for (image, length) in missing {
            let (bytes, mime, width, height) = match image.read_checked(length) {
                Ok(value) => value,
                Err(error) if image.grant.is_some() => {
                    reserved.bytes -= length;
                    self.budget.used.fetch_sub(length, Ordering::AcqRel);
                    failures.insert(
                        image.token.clone(),
                        file_media::failure(
                            error.status(),
                            "media_file_invalid",
                            &error.to_string(),
                        ),
                    );
                    continue;
                }
                Err(error) => return Err(error),
            };
            reserved.bytes -= length;
            let blob = Arc::new(MediaBlob {
                bytes,
                mime,
                width,
                height,
                _charge: Charge {
                    budget: self.budget.clone(),
                    bytes: length,
                },
            });
            staged.push((image, blob));
        }
        for (image, blob) in staged {
            cache.entries.insert(
                image.token.clone(),
                Entry {
                    source: image.source_weak(),
                    grant: image.grant.clone(),
                    native_scope: image.native_scope.clone(),
                    blob,
                    used: 0,
                },
            );
        }
        let mut projected = Vec::with_capacity(images.len());
        for image in images {
            if let Some(error) = failures.get(&image.token) {
                projected.push(error.clone());
                continue;
            }
            let used = tick(&mut cache);
            let entry = cache
                .entries
                .get_mut(&image.token)
                .ok_or(MediaError::Unavailable)?;
            entry.used = used;
            projected.push(json!({"src":format!("/api/media/{}", image.token),"mime":entry.blob.mime(),"alt":"会话图片","width":entry.blob.width,"height":entry.blob.height}));
        }
        Ok(projected)
    }
    /// Token lookup is bounded and performs no filesystem access. Call off the
    /// reactor: projection may currently own the cache lock for decoding.
    #[cfg(test)]
    pub fn get(&self, token: &str) -> Option<Arc<MediaBlob>> {
        if token.len() != 32
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        let mut cache = self.cache.lock().ok()?;
        let used = tick(&mut cache);
        let entry = cache.entries.get_mut(token)?;
        if entry.grant.is_some() || entry.native_scope.is_some() {
            return None;
        }
        entry.used = used;
        Some(entry.blob.clone())
    }
    #[cfg(test)]
    pub(crate) fn file_ticket(&self, token: &str) -> Option<FileTicket> {
        if token.len() != 32
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        let mut cache = self.cache.lock().ok()?;
        let used = tick(&mut cache);
        let entry = cache.entries.get_mut(token)?;
        let grant = entry.grant.clone()?;
        entry.used = used;
        Some(FileTicket::new(grant, entry.blob.clone()))
    }
}

fn inspect(mime: Mime, bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    match mime {
        Mime::Png => png(bytes),
        Mime::Jpeg => jpeg(bytes),
        mime => formats::inspect(mime.text(), bytes),
    }
}
fn tick(cache: &mut Cache) -> u64 {
    if cache.clock == u64::MAX {
        let mut order = cache
            .entries
            .iter()
            .map(|(token, entry)| (entry.used, token.clone()))
            .collect::<Vec<_>>();
        order.sort();
        for (index, (_, token)) in order.into_iter().enumerate() {
            cache
                .entries
                .get_mut(&token)
                .expect("existing cache entry")
                .used = index as u64;
        }
        cache.clock = cache.entries.len() as u64;
    }
    cache.clock += 1;
    cache.clock
}

fn dimensions(width: u32, height: u32) -> Result<(u32, u32), MediaError> {
    if width == 0 || height == 0 {
        return Err(MediaError::Invalid);
    }
    Ok((width, height))
}
fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("checked four-byte field"))
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
/// PNG container/header validation, not zlib decompression or pixel decoding.
fn png(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    png_base(bytes, false)
}
/// Only the private APNG wrapper may ignore animation chunks, after validating
/// their complete ordering, frame budgets and CRCs. Borrow the original buffer:
/// ignored chunks must not terminate a run of IDAT chunks as filtering did not.
fn png_base(bytes: &[u8], animation_validated: bool) -> Result<(u32, u32), MediaError> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(MediaError::Invalid);
    }
    let mut position = 8usize;
    let mut size = None;
    let mut image_data = false;
    let mut data_ended = false;
    let mut palette = false;
    let mut indexed = false;
    while position < bytes.len() {
        let header = bytes
            .get(position..position + 8)
            .ok_or(MediaError::Invalid)?;
        let length = be32(&header[..4]) as usize;
        let end = position
            .checked_add(12)
            .and_then(|n| n.checked_add(length))
            .filter(|&end| end <= bytes.len())
            .ok_or(MediaError::Invalid)?;
        let kind = &header[4..8];
        if !kind.iter().all(u8::is_ascii_alphabetic) || !kind[2].is_ascii_uppercase() {
            return Err(MediaError::Invalid);
        }
        let payload = &bytes[position + 8..end - 4];
        if crc32(&bytes[position + 4..end - 4]) != be32(&bytes[end - 4..end]) {
            return Err(MediaError::Invalid);
        }
        if animation_validated && matches!(kind, b"acTL" | b"fcTL" | b"fdAT") {
            position = end;
            continue;
        }
        if size.is_none() && kind != b"IHDR" {
            return Err(MediaError::Invalid);
        }
        if image_data && kind != b"IDAT" {
            data_ended = true;
        }
        match kind {
            b"IHDR" => {
                if size.is_some() || length != 13 {
                    return Err(MediaError::Invalid);
                }
                let valid_depth = match payload[9] {
                    0 => matches!(payload[8], 1 | 2 | 4 | 8 | 16),
                    2 | 4 | 6 => matches!(payload[8], 8 | 16),
                    3 => matches!(payload[8], 1 | 2 | 4 | 8),
                    _ => false,
                };
                if !valid_depth || payload[10] != 0 || payload[11] != 0 || payload[12] > 1 {
                    return Err(MediaError::Invalid);
                }
                indexed = payload[9] == 3;
                size = Some(dimensions(be32(&payload[..4]), be32(&payload[4..8]))?);
            }
            b"PLTE" => {
                if palette || image_data || length == 0 || length > 768 || !length.is_multiple_of(3)
                {
                    return Err(MediaError::Invalid);
                }
                palette = true;
            }
            b"IDAT" => {
                if data_ended || (indexed && !palette) {
                    return Err(MediaError::Invalid);
                }
                image_data |= length > 0;
            }
            b"IEND" => {
                if length != 0 || !image_data || end != bytes.len() {
                    return Err(MediaError::Invalid);
                }
                return size.ok_or(MediaError::Invalid);
            }
            b"acTL" | b"fcTL" | b"fdAT" => return Err(MediaError::Unsupported),
            _ if kind[0].is_ascii_uppercase() => return Err(MediaError::Unsupported),
            _ => {}
        }
        position = end;
    }
    Err(MediaError::Invalid)
}
/// Baseline/progressive JPEG marker validation. Entropy data is bounded but is
/// not Huffman-decoded; successful validation does not certify pixel contents.
fn jpeg(bytes: &[u8]) -> Result<(u32, u32), MediaError> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return Err(MediaError::Invalid);
    }
    let mut position = 2usize;
    let mut size = None;
    let mut components = 0usize;
    let mut scan = false;
    let mut quantization = false;
    let mut huffman = false;
    loop {
        if bytes.get(position) != Some(&0xff) {
            return Err(MediaError::Invalid);
        }
        while bytes.get(position) == Some(&0xff) {
            position += 1;
        }
        let marker = *bytes.get(position).ok_or(MediaError::Invalid)?;
        position += 1;
        if marker == 0xd9 {
            return if scan && quantization && huffman && position == bytes.len() {
                size.ok_or(MediaError::Invalid)
            } else {
                Err(MediaError::Invalid)
            };
        }
        if matches!(marker, 0x00 | 0xd0..=0xd8) {
            return Err(MediaError::Invalid);
        }
        let length_bytes = bytes
            .get(position..position + 2)
            .ok_or(MediaError::Invalid)?;
        let length = u16::from_be_bytes([length_bytes[0], length_bytes[1]]) as usize;
        if length < 2 {
            return Err(MediaError::Invalid);
        }
        let end = position
            .checked_add(length)
            .filter(|&end| end <= bytes.len())
            .ok_or(MediaError::Invalid)?;
        let payload = &bytes[position + 2..end];
        position = end;
        match marker {
            0xc0 | 0xc2 => {
                if size.is_some() || payload.len() < 6 || payload[0] != 8 {
                    return Err(MediaError::Invalid);
                }
                components = payload[5] as usize;
                if !matches!(components, 1 | 3 | 4) || payload.len() != 6 + 3 * components {
                    return Err(MediaError::Invalid);
                }
                size = Some(dimensions(
                    u32::from(u16::from_be_bytes([payload[3], payload[4]])),
                    u32::from(u16::from_be_bytes([payload[1], payload[2]])),
                )?);
            }
            0xc1 | 0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                return Err(MediaError::Unsupported);
            }
            0xdb => {
                if payload.len() < 65 {
                    return Err(MediaError::Invalid);
                }
                quantization = true;
            }
            0xc4 => {
                if payload.len() < 18 {
                    return Err(MediaError::Invalid);
                }
                huffman = true;
            }
            0xda => {
                if size.is_none()
                    || payload.is_empty()
                    || payload[0] == 0
                    || payload[0] as usize > components
                    || payload.len() != 4 + 2 * payload[0] as usize
                {
                    return Err(MediaError::Invalid);
                }
                let start = position;
                loop {
                    let byte = *bytes.get(position).ok_or(MediaError::Invalid)?;
                    if byte != 0xff {
                        position += 1;
                        continue;
                    }
                    let next = *bytes.get(position + 1).ok_or(MediaError::Invalid)?;
                    if next == 0x00 || matches!(next, 0xd0..=0xd7) {
                        position += 2;
                        continue;
                    }
                    if next == 0xff {
                        position += 1;
                        continue;
                    }
                    break;
                }
                if position == start {
                    return Err(MediaError::Invalid);
                }
                scan = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
