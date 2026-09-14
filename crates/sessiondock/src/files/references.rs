use super::{FileError, FileScope, MAX_PATH_BYTES};
use regex::Regex;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

static LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?::[0-9]+(?::[0-9]+)?|#L[0-9]+(?:C[0-9]+)?)$").unwrap());
static QUOTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"`([^`\n]+)`|\]\(\s*(<[^>]+>|(?:[^\s()]|\([^\s()]*\))+)(?:\s+["'][^"']*["'])?\s*\)"#,
    )
    .unwrap()
});
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:https?://[^\s<>`"']+|(?:~/|\.\.?/|/|[a-z0-9_.-]+/)[^\s<>`"'，。；、！？()\[\]{}]+|[a-z0-9_.-]+\.[a-zA-Z][\w.-]*(?::[0-9]+(?::[0-9]+)?|#L[0-9]+(?:C[0-9]+)?)?)"#).unwrap()
});
// Media discovery accepts Unicode names and drive paths. Scan the entire
// selected branch here, rather than reusing discovery's 16-item display cap:
// later complete paths remain necessary evidence for basename ambiguity.
static MEDIA_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:(?:https?|file)://[^\s<>`"']+|[a-z]:[/\\][^\s<>`"'，。；、！？()\[\]{}]+|(?:~/|\.\.?/|/|[^\s<>`"'，。；、！？()\[\]{}\\/:]+/)[^\s<>`"'，。；、！？()\[\]{}]+|[^\s<>`"'，。；、！？()\[\]{}\\/:]+\.[a-zA-Z][\w.-]*)"#).unwrap()
});
static FILE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bfile://[^\s<>`\"'，。；、！？()\[\]{}]+"#).unwrap());

/// Decode a local media path.
pub(crate) fn normalize_media_ref(raw: &str) -> Result<String, FileError> {
    let raw = raw.trim().trim_matches(['<', '>']);
    if raw.is_empty() {
        return Err(FileError::new(
            400,
            "file_media_reference_invalid",
            "图片路径不能为空",
        ));
    }
    let bytes = raw.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let (Some(high), Some(low)) = (bytes.get(index + 1), bytes.get(index + 2))
            && let (Some(high), Some(low)) =
                ((*high as char).to_digit(16), (*low as char).to_digit(16))
        {
            decoded.push((high * 16 + low) as u8);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    let decoded = String::from_utf8_lossy(&decoded);
    let value = if let Some(rest) = decoded.strip_prefix("file://") {
        #[cfg(windows)]
        let windows_path = rest.starts_with('\\')
            || (rest.as_bytes().get(1) == Some(&b':') && rest.as_bytes()[0].is_ascii_alphabetic());
        #[cfg(not(windows))]
        let windows_path = false;
        let path = if rest.starts_with('/') || windows_path {
            rest
        } else {
            rest.find('/').map_or("", |slash| &rest[slash..])
        };
        #[cfg(windows)]
        let verbatim = path.starts_with("\\\\?\\");
        #[cfg(not(windows))]
        let verbatim = false;
        if verbatim {
            path
        } else {
            path.split(['?', '#']).next().unwrap_or("")
        }
    } else {
        &decoded
    };
    #[cfg(windows)]
    {
        let value = value
            .strip_prefix("\\\\?\\UNC\\")
            .map(|rest| format!("//{rest}"))
            .or_else(|| value.strip_prefix("\\\\?\\").map(str::to_owned))
            .unwrap_or_else(|| value.to_owned());
        let value = value
            .strip_prefix('/')
            .filter(|value| value.as_bytes().get(1) == Some(&b':'))
            .unwrap_or(&value);
        return Ok(value.replace('\\', "/"));
    }
    #[cfg(not(windows))]
    Ok(value.to_owned())
}

pub fn clean_ref(value: &str) -> Result<String, FileError> {
    if value.is_empty() || value.chars().count() > MAX_PATH_BYTES || value.contains('\0') {
        return Err(FileError::new(
            400,
            "file_reference_invalid",
            "文件引用为空、过长或包含空字符",
        ));
    }
    let clean = LINE
        .replace(value.trim().trim_matches(['<', '>']), "")
        .into_owned();
    if clean.is_empty() {
        return Err(FileError::new(
            400,
            "file_reference_invalid",
            "文件引用不能为空",
        ));
    }
    Ok(clean)
}
pub(super) fn validate_scope(scope: &FileScope<'_>) -> Result<(), FileError> {
    if scope.uid.is_empty() {
        return Err(FileError::new(
            400,
            "file_scope_invalid",
            "文件访问需要已解析的有效会话视图",
        ));
    }
    Ok(())
}

pub(super) struct ReferenceIndex {
    pub refs: BTreeSet<String>,
    pub basenames: BTreeMap<String, Vec<String>>,
    pub directories: Vec<String>,
    media: bool,
    /// Complete selected-branch scan marker retained for media integration.
    /// The index no longer truncates at an arbitrary reference budget; all
    /// simply unknown (no placeholder), never a scope-wide failure.
    pub complete: bool,
}
impl ReferenceIndex {
    pub fn new(messages: &[Value]) -> Result<Self, FileError> {
        Self::build(messages, false, &[])
    }
    pub(super) fn media<'a>(
        messages: impl IntoIterator<Item = &'a Value>,
        native_refs: &[&str],
    ) -> Result<Self, FileError> {
        Self::build(messages, true, native_refs)
    }
    fn build<'a>(
        messages: impl IntoIterator<Item = &'a Value>,
        media: bool,
        native_refs: &[&str],
    ) -> Result<Self, FileError> {
        let mut index = Self {
            refs: BTreeSet::new(),
            basenames: BTreeMap::new(),
            directories: Vec::new(),
            media,
            complete: true,
        };
        for message in messages {
            if !index.complete {
                break;
            }
            // Caller supplies semantic messages, never raw metadata/native records.
            let role = message["role"].as_str().unwrap_or("");
            let native_role = if media {
                role.strip_suffix("·subagent").unwrap_or(role)
            } else {
                role
            };
            if !matches!(
                native_role,
                "user" | "assistant" | "tool" | "tool_result" | "command" | "thinking"
            ) && !(media && role == "·subagent")
            {
                continue;
            }
            if let Some(text) = message["text"].as_str() {
                index.scan(text)?;
                if native_role == "tool"
                    && let Ok(value) = serde_json::from_str::<Value>(text)
                {
                    index.scan_value(&value, 0)?;
                }
            }
            if native_role == "tool" {
                index.scan_value(&message["args"], 0)?;
            }
        }
        for reference in native_refs {
            // Exact typed refs from the selected native view, not fake text
            // fed through the token scanner. Invalid refs cannot grant access;
            // image(ref) reports their precise error if actually selected.
            index.insert(reference)?;
        }
        for reference in &index.refs {
            let basename = reference.rsplit(['/', '\\']).next().unwrap_or(reference);
            index
                .basenames
                .entry(basename.to_owned())
                .or_default()
                .push(reference.clone());
            if reference.contains(['/', '\\']) {
                index.directories.push(reference.clone());
            }
        }
        Ok(index)
    }
    fn insert(&mut self, value: &str) -> Result<(), FileError> {
        if value.to_ascii_lowercase().starts_with("http://")
            || value.to_ascii_lowercase().starts_with("https://")
        {
            return Ok(());
        }
        if let Ok(reference) = if self.media {
            normalize_media_ref(value)
        } else {
            clean_ref(value)
        } {
            self.refs.insert(reference);
        }
        Ok(())
    }
    fn scan(&mut self, value: &str) -> Result<(), FileError> {
        if !self.complete {
            return Ok(());
        }
        if self.media {
            for url in FILE_URL.find_iter(value) {
                self.insert(url.as_str().trim_end_matches(['.', ',', ';', '!']))?;
            }
        }
        if value.starts_with(['/', '\\'])
            || value.starts_with("~/")
            || value.starts_with("./")
            || value.starts_with("../")
            || (value.as_bytes().get(1) == Some(&b':'))
        {
            self.insert(value)?;
        }
        for capture in QUOTED.captures_iter(value) {
            let raw = capture.get(1).or_else(|| capture.get(2)).unwrap().as_str();
            let raw = if self.media && capture.get(2).is_some() {
                raw.strip_prefix('<')
                    .and_then(|raw| raw.strip_suffix('>'))
                    .unwrap_or(raw)
            } else {
                raw
            };
            self.insert(raw)?;
        }
        let tokens = if self.media { &*MEDIA_TOKEN } else { &*TOKEN };
        for capture in tokens.find_iter(value) {
            let preceding = value[..capture.start()].chars().next_back();
            if preceding.is_some_and(|c| c.is_ascii_alphanumeric() || "_@/:.-".contains(c)) {
                continue;
            }
            self.insert(
                capture
                    .as_str()
                    .trim_end_matches(['.', ',', ';', ':', '!', '?']),
            )?;
            if !self.complete {
                return Ok(());
            }
        }
        Ok(())
    }

    fn scan_value(&mut self, value: &Value, _depth: usize) -> Result<(), FileError> {
        let mut pending = vec![value];
        while let Some(value) = pending.pop() {
            match value {
                Value::String(value) => self.scan(value)?,
                Value::Array(values) => pending.extend(values.iter()),
                Value::Object(values) => pending.extend(values.values()),
                _ => {}
            }
        }
        Ok(())
    }
}
