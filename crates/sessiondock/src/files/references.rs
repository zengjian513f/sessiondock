use super::{FileError, FileScope, MAX_PATH_BYTES};
use regex::Regex;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

/// Scan budgets per selected view. The largest real sessions observed (a
/// 228 MB Codex rollout, a 174 MB fork chain) carry about 41 MB of semantic
/// text and 47,000 unique references under the Python `files.references`
/// rules, so both limits keep an order of magnitude of headroom; Python itself
/// has no limit. `resolve-files`/file-page requests still report 413 beyond
/// them; the media index degrades instead (`ReferenceIndex::complete`).
pub(crate) const MAX_REFERENCE_BYTES: usize = 512 * 1024 * 1024;
pub(crate) const MAX_REFERENCES: usize = 250_000;
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

/// A local image reference is still only a reference, not a filesystem grant.
/// URL decoding occurs exactly once and only for file:/// URLs. Ordinary path
/// percent signs are literal. Platform/no-follow/root checks remain downstream.
pub(crate) fn normalize_media_ref(raw: &str) -> Result<String, FileError> {
    let invalid = || {
        FileError::new(
            400,
            "file_media_reference_invalid",
            "媒体文件引用需要本地路径或无 authority 的 file:/// URL；不展开 HOME、网络地址或控制字符",
        )
    };
    if raw.is_empty() || raw.len() > MAX_PATH_BYTES || raw.chars().any(char::is_control) {
        return Err(invalid());
    }
    // Markdown delimiters are removed by the reference lexer, not here. A
    // decoded filename may legitimately end in a space or `>` on Unix.
    let value = raw;
    if value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
    {
        let path = &value[7..];
        if !path.starts_with('/') || path.starts_with("//") || path.contains(['?', '#', '\\']) {
            return Err(invalid());
        }
        let mut decoded = Vec::with_capacity(path.len());
        let mut bytes = path.bytes();
        while let Some(byte) = bytes.next() {
            if byte == b'%' {
                let high = bytes
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or_else(invalid)?;
                let low = bytes
                    .next()
                    .and_then(|b| (b as char).to_digit(16))
                    .ok_or_else(invalid)?;
                decoded.push(((high << 4) | low) as u8);
            } else {
                decoded.push(byte);
            }
        }
        let decoded = String::from_utf8(decoded).map_err(|_| invalid())?;
        if decoded.chars().any(char::is_control)
            || decoded.starts_with("//")
            || decoded.contains('\\')
            || decoded.contains("://")
        {
            return Err(invalid());
        }
        #[cfg(windows)]
        let decoded = if decoded.as_bytes().get(2) == Some(&b':')
            && decoded
                .as_bytes()
                .get(1)
                .is_some_and(u8::is_ascii_alphabetic)
        {
            decoded[1..].to_owned()
        } else {
            return Err(invalid());
        };
        return Ok(decoded);
    }
    if value.starts_with('~')
        || value.contains("://")
        || value
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        return Err(invalid());
    }
    if value.is_empty() {
        return Err(invalid());
    }
    // Unlike source-file links, image references are exact filenames. Stripping
    // :line/#L suffixes here would corrupt decoded names such as image%23L2 and
    // would make token reauthorization normalize the same path differently.
    Ok(value.to_owned())
}

pub fn clean_ref(value: &str) -> Result<String, FileError> {
    if value.is_empty() || value.len() > MAX_PATH_BYTES || value.chars().any(char::is_control) {
        return Err(FileError::new(
            400,
            "file_reference_invalid",
            "文件引用为空、过长或包含控制字符",
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
    if scope.uid.is_empty()
        || scope.uid.len() > 256
        || scope.uid.chars().any(char::is_control)
        || scope
            .agent
            .is_some_and(|agent| agent.len() > 256 || agent.chars().any(char::is_control))
    {
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
    scanned: usize,
    media: bool,
    /// False once a media-mode scan stopped at a budget: later references are
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
            scanned: 0,
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
            index.charge(reference.len())?;
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
        if self.refs.len() > MAX_REFERENCES {
            return self.exhausted(FileError::new(
                413,
                "file_reference_budget",
                "所选历史中的文件引用过多，无法安全完成解析",
            ));
        }
        Ok(())
    }
    /// Media scans degrade (stop, keep what was indexed); file resolution
    /// keeps the explicit error so a user-initiated open is never guessed.
    fn exhausted(&mut self, error: FileError) -> Result<(), FileError> {
        if self.media {
            self.complete = false;
            Ok(())
        } else {
            Err(error)
        }
    }
    fn scan(&mut self, value: &str) -> Result<(), FileError> {
        self.charge(value.len())?;
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
    fn charge(&mut self, bytes: usize) -> Result<(), FileError> {
        self.scanned = self.scanned.saturating_add(bytes);
        if self.scanned > MAX_REFERENCE_BYTES {
            return self.exhausted(FileError::new(
                413,
                "file_reference_budget",
                "所选历史的文件引用解析超过 512 MiB 预算",
            ));
        }
        Ok(())
    }
    fn scan_value(&mut self, value: &Value, depth: usize) -> Result<(), FileError> {
        if depth > 32 {
            return Err(FileError::new(
                413,
                "file_reference_depth",
                "工具参数嵌套过深，不能完整解析文件引用",
            ));
        }
        match value {
            Value::String(value) => self.scan(value)?,
            Value::Array(values) => {
                for value in values {
                    self.scan_value(value, depth + 1)?;
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    self.scan_value(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
