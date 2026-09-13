use super::{
    FileError, ListOptions, MAX_PREVIEW_BYTES, MAX_RAW_BYTES, MAX_STREAM_BYTES, ResolvedTarget,
    STREAM_CHUNK_BYTES, boundary,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Default, Debug, Clone)]
pub struct ReadOptions {
    pub download: bool,
    pub preview: bool,
    pub head: bool,
    pub range: Option<String>,
}
impl ReadOptions {
    fn effective_range(&self) -> Option<&str> {
        if self.head {
            None
        } else {
            self.range.as_deref()
        }
    }
}
pub struct FileResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: FileBody,
}
pub enum FileBody {
    Empty,
    Bytes(Vec<u8>),
    Reader(Box<CheckedReader>),
}

/// Read only this handle in a bounded blocking worker, never reopen its path.
/// A short native read or changed metadata is an error, not a successful EOF.
pub struct CheckedReader {
    target: ResolvedTarget,
    remaining: u64,
}
impl CheckedReader {
    pub fn remaining(&self) -> u64 {
        self.remaining
    }
}
impl Read for CheckedReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        self.target.verify().map_err(std::io::Error::other)?;
        if self.remaining == 0 {
            return Ok(0);
        }
        let count = output
            .len()
            .min(self.remaining.min(STREAM_CHUNK_BYTES as u64) as usize);
        let mut file = self.target.file().map_err(std::io::Error::other)?;
        let read = file.read(&mut output[..count])?;
        self.target.verify().map_err(std::io::Error::other)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "授权文件在传输完成前结束",
            ));
        }
        self.remaining -= read as u64;
        Ok(read)
    }
}

/// Authorized, single-consumer image input. The extension is only a MIME hint;
/// callers validate actual bytes and retain this handle through the final check.
pub(crate) struct CheckedImage {
    reader: CheckedReader,
    version: super::FileVersion,
    size: u64,
    mime_hint: Option<&'static str>,
}
impl CheckedImage {
    pub(crate) fn new(target: ResolvedTarget) -> Result<Self, FileError> {
        target.verify()?;
        if target.kind() != "file" {
            return Err(FileError::new(
                400,
                "file_image_required",
                "图片引用必须指向普通文件，不能指向目录",
            ));
        }
        let size = target.metadata.len();
        if size > MAX_RAW_BYTES {
            return Err(FileError::new(
                413,
                "file_image_budget",
                "单张磁盘图片超过 32 MiB 读取上限",
            ));
        }
        let version = target.version();
        let mime_hint = media(target.path()).filter(|mime| mime.starts_with("image/"));
        let mut file = target.file()?;
        file.seek(SeekFrom::Start(0)).map_err(FileError::io)?;
        target.verify()?;
        Ok(Self {
            reader: CheckedReader {
                target,
                remaining: size,
            },
            version,
            size,
            mime_hint,
        })
    }
    pub(crate) fn remaining(&self) -> u64 {
        self.reader.remaining()
    }
    pub(crate) fn size(&self) -> u64 {
        self.size
    }
    pub(crate) fn version(&self) -> &super::FileVersion {
        &self.version
    }
    pub(crate) fn mime_hint(&self) -> Option<&'static str> {
        self.mime_hint
    }
    pub(crate) fn verify(&self) -> Result<(), FileError> {
        self.reader.target.verify()
    }
}
impl Read for CheckedImage {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            self.verify().map_err(std::io::Error::other)?;
        }
        self.reader.read(output)
    }
}

fn media(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "bmp" => Some("image/bmp"),
        "mp4" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "ogg" => Some("audio/ogg"),
        "m4a" => Some("audio/mp4"),
        "flac" => Some("audio/flac"),
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}
fn text(data: &[u8], truncated: bool) -> Option<String> {
    if data.contains(&0) {
        return None;
    }
    match std::str::from_utf8(data) {
        Ok(text) => Some(text.into()),
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&data[..error.valid_up_to()])
                .ok()
                .map(str::to_owned)
        }
        _ => None,
    }
}
fn prefix(target: &ResolvedTarget, limit: usize) -> Result<Vec<u8>, FileError> {
    target.verify()?;
    let mut file = target.file()?;
    file.seek(SeekFrom::Start(0)).map_err(FileError::io)?;
    let expected = target.metadata.len().min(limit as u64) as usize;
    let mut bytes = vec![0; expected];
    file.read_exact(&mut bytes).map_err(FileError::io)?;
    target.verify()?;
    file.seek(SeekFrom::Start(0)).map_err(FileError::io)?;
    Ok(bytes)
}
fn validate_pdf(target: &ResolvedTarget) -> Result<(), FileError> {
    let data = prefix(target, 1024)?;
    if !data.windows(5).any(|bytes| bytes == b"%PDF-") {
        return Err(FileError::new(
            400,
            "file_invalid_pdf",
            "文件没有有效的 PDF 标识，请下载后检查",
        ));
    }
    Ok(())
}
pub(super) fn describe(target: &ResolvedTarget) -> Result<Value, FileError> {
    target.verify()?;
    let name = target
        .path()
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let mut result = json!({"name":name,"path":boundary::wire_path(target.path())?,"size":target.metadata.len(),"modified":boundary::modified(&target.metadata),"kind":target.kind(),"mime":mime_guess::from_path(target.path()).first_raw().unwrap_or("application/octet-stream"),"mode":Value::Null,"owner":Value::Null,"group":Value::Null,"writable":false});
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        let mode = target.metadata.mode();
        let mut label = String::from(if target.kind() == "directory" {
            "d"
        } else {
            "-"
        });
        for (bit, character) in [
            (0o400, 'r'),
            (0o200, 'w'),
            (0o100, 'x'),
            (0o040, 'r'),
            (0o020, 'w'),
            (0o010, 'x'),
            (0o004, 'r'),
            (0o002, 'w'),
            (0o001, 'x'),
        ] {
            label.push(if mode & bit != 0 { character } else { '-' });
        }
        result["mode"] = json!(label);
        result["owner"] = json!(target.metadata.uid());
        result["group"] = json!(target.metadata.gid());
    }
    if target.kind() == "file" {
        if let Some(mime) = media(target.path()) {
            if mime == "application/pdf" {
                validate_pdf(target)?;
            }
            result["preview"] = json!(mime);
        } else {
            let data = prefix(target, MAX_PREVIEW_BYTES)?;
            let truncated = target.metadata.len() > MAX_PREVIEW_BYTES as u64;
            if let Some(text) = text(&data, truncated) {
                result["preview"] = json!("text");
                result["text"] = json!(text);
                result["truncated"] = json!(truncated);
            } else {
                result["preview"] = json!("unsupported");
            }
        }
    }
    target.verify()?;
    Ok(result)
}

fn disposition(name: &str, download: bool) -> String {
    let safe: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || ".-_".contains(character) {
                character
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(byte) {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!(
        "{}; filename=\"{}\"; filename*=UTF-8''{}",
        if download { "attachment" } else { "inline" },
        safe,
        encoded
    )
}
fn headers(mime: &str, name: &str, attachment: bool, length: u64) -> BTreeMap<String, String> {
    [
        ("Content-Type", mime.to_owned()),
        ("Content-Length", length.to_string()),
        ("Content-Disposition", disposition(name, attachment)),
        ("Cache-Control", "no-store".into()),
        ("X-Content-Type-Options", "nosniff".into()),
        (
            "Content-Security-Policy",
            if mime == "application/pdf" && !attachment {
                "frame-ancestors 'self'"
            } else {
                "sandbox; default-src 'none'"
            }
            .into(),
        ),
        ("Referrer-Policy", "no-referrer".into()),
        ("Accept-Ranges", "bytes".into()),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value))
    .collect()
}

/// Strict single byte range. Multipart, signs, whitespace and empty/suffix-zero
/// ranges are rejected with 416. Overshooting an existing end is clipped.
fn range(raw: Option<&str>, size: u64) -> Result<(u16, u64, u64), ()> {
    let Some(raw) = raw else {
        return Ok((200, 0, size));
    };
    let (unit, body) = raw.split_once('=').ok_or(())?;
    // HTTP only defines bytes here; unknown range units must be ignored.
    if !unit.eq_ignore_ascii_case("bytes") {
        return Ok((200, 0, size));
    }
    let (left, right) = body.split_once('-').ok_or(())?;
    if size == 0
        || (left.is_empty() && right.is_empty())
        || !left
            .bytes()
            .chain(right.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(());
    }
    if left.is_empty() {
        let suffix: u64 = right.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let length = size.min(suffix);
        return Ok((206, size - length, length));
    }
    let start: u64 = left.parse().map_err(|_| ())?;
    let end = if right.is_empty() {
        size - 1
    } else {
        right.parse::<u64>().map_err(|_| ())?.min(size - 1)
    };
    if start >= size || start > end {
        return Err(());
    }
    Ok((206, start, end - start + 1))
}

pub(super) fn read(
    target: ResolvedTarget,
    options: &ReadOptions,
) -> Result<FileResponse, FileError> {
    target.verify()?;
    if target.kind() == "directory" {
        if options.download || options.preview {
            return Err(FileError::new(
                400,
                "file_required",
                "目录只提供有界列表，不能作为文件预览或下载",
            ));
        }
        let value = boundary::list(&target, &ListOptions::default())?;
        let mut listing = format!("{}/\n\n", boundary::wire_path(target.path())?);
        for entry in value["entries"].as_array().unwrap() {
            listing.push_str(entry["name"].as_str().unwrap_or(""));
            if entry["kind"] == "directory" {
                listing.push('/');
            }
            listing.push('\n');
        }
        if value["next_offset"].is_number() {
            listing.push_str("…（请使用目录浏览分页查看其余项目）\n");
        }
        let data = listing.into_bytes();
        return bytes_response(
            data,
            "text/plain; charset=utf-8",
            "directory.txt",
            false,
            options,
        );
    }
    let size = target.metadata.len();
    if size > MAX_STREAM_BYTES {
        return Err(FileError::new(
            413,
            "file_stream_budget",
            "单文件超过 16 GiB 开发传输上限",
        ));
    }
    let name = target
        .path()
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download")
        .to_owned();
    let (mime, attachment) = if options.download {
        ("application/octet-stream", true)
    } else if let Some(mime) = media(target.path()) {
        if !options.preview && size > MAX_RAW_BYTES {
            return Err(FileError::new(
                413,
                "file_raw_budget",
                "原始打开超过 32 MiB，请使用预览或下载",
            ));
        }
        if mime == "application/pdf" {
            validate_pdf(&target)?;
        }
        (mime, false)
    } else if options.preview {
        return Err(FileError::new(
            400,
            "file_preview_unsupported",
            "此格式请使用文本预览或下载",
        ));
    } else {
        if size > MAX_RAW_BYTES {
            return Err(FileError::new(
                413,
                "file_raw_budget",
                "原始打开超过 32 MiB，请使用文本预览或下载",
            ));
        }
        let data = prefix(&target, MAX_RAW_BYTES as usize)?;
        let is_text = text(&data, false).is_some();
        return bytes_response(
            data,
            if is_text {
                "text/plain; charset=utf-8"
            } else {
                "application/octet-stream"
            },
            &name,
            !is_text,
            options,
        );
    };
    let mut headers = headers(mime, &name, attachment, size);
    let (status, start, length) = match range(options.effective_range(), size) {
        Ok(value) => value,
        Err(()) => {
            headers.insert("Content-Range".into(), format!("bytes */{size}"));
            headers.insert("Content-Length".into(), "0".into());
            return Ok(FileResponse {
                status: 416,
                headers,
                body: FileBody::Empty,
            });
        }
    };
    headers.insert("Content-Length".into(), length.to_string());
    if status == 206 {
        headers.insert(
            "Content-Range".into(),
            format!("bytes {start}-{}/{size}", start + length - 1),
        );
    }
    target.verify()?;
    let mut file = target.file()?;
    file.seek(SeekFrom::Start(start)).map_err(FileError::io)?;
    Ok(FileResponse {
        status,
        headers,
        body: if options.head {
            FileBody::Empty
        } else {
            FileBody::Reader(Box::new(CheckedReader {
                target,
                remaining: length,
            }))
        },
    })
}
fn bytes_response(
    data: Vec<u8>,
    mime: &str,
    name: &str,
    attachment: bool,
    options: &ReadOptions,
) -> Result<FileResponse, FileError> {
    let size = data.len() as u64;
    let mut headers = headers(mime, name, attachment, size);
    let (status, start, length) = match range(options.effective_range(), size) {
        Ok(value) => value,
        Err(()) => {
            headers.insert("Content-Range".into(), format!("bytes */{size}"));
            headers.insert("Content-Length".into(), "0".into());
            return Ok(FileResponse {
                status: 416,
                headers,
                body: FileBody::Empty,
            });
        }
    };
    headers.insert("Content-Length".into(), length.to_string());
    if status == 206 {
        headers.insert(
            "Content-Range".into(),
            format!("bytes {start}-{}/{size}", start + length - 1),
        );
    }
    Ok(FileResponse {
        status,
        headers,
        body: if options.head {
            FileBody::Empty
        } else if status == 200 {
            FileBody::Bytes(data)
        } else {
            FileBody::Bytes(data[start as usize..(start + length) as usize].to_vec())
        },
    })
}
