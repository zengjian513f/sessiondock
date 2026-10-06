//! Pure text discovery only. A candidate is not filesystem authority.
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) struct Discovered {
    pub reference: String,
    pub gallery: bool,
}

pub(crate) fn discover(message: &Value) -> Vec<Discovered> {
    let Some(text) = message.get("text").and_then(Value::as_str) else {
        return Vec::new();
    };
    let role = message.get("role").and_then(Value::as_str).unwrap_or("");
    let raw = matches!(
        role,
        "user"
            | "assistant"
            | "command"
            | "·subagent"
            | "user·subagent"
            | "assistant·subagent"
            | "command·subagent"
    );
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut fence = None;
    for line in text.lines() {
        if let Some((marker, count, tail)) = fence_line(line) {
            match fence {
                None => fence = Some((marker, count)),
                Some((open, n)) if marker == open && count >= n && tail.trim().is_empty() => {
                    fence = None
                }
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let mut position = 0;
        let mut spans = Vec::new();
        while let Some(relative) = line[position..].find("![") {
            let start = position + relative;
            position = start + 2;
            if start > 0 && line.as_bytes()[start - 1] == b'\\' {
                continue;
            }
            if let Some((reference, end)) = markdown(line, start) {
                spans.push((start, end));
                position = end;
                insert(&mut found, &mut seen, reference, false);
            }
        }
        if raw {
            // Markdown destinations (including rejected remote ones) are not also
            // gallery candidates. Split only the ordinary text around images.
            let mut start = 0;
            for (a, b) in spans
                .into_iter()
                .chain(std::iter::once((line.len(), line.len())))
            {
                for token in line[start..a].split_whitespace() {
                    let reference = token.trim_matches(|c: char| {
                        matches!(
                            c,
                            '"' | '\''
                                | '`'
                                | '<'
                                | '>'
                                | '('
                                | ')'
                                | '['
                                | ']'
                                | '{'
                                | '}'
                                | ','
                                | ';'
                        )
                    });
                    let reference = reference.trim_end_matches(['.', '!', '。', '，', '；']);
                    if !raw_path_shape(reference) {
                        continue;
                    }
                    insert(&mut found, &mut seen, reference, true);
                }
                start = b;
            }
        }
    }
    found
}
/// Only `~`, `/`, `./` and `../` prefixed tokens are treated as
/// image paths in prose (a Windows drive spelling is the Rust equivalent); a
/// bare `shot.png` in a sentence is text, not a media reference.
fn raw_path_shape(reference: &str) -> bool {
    reference.starts_with(['/', '~'])
        || reference.starts_with("./")
        || reference.starts_with("../")
        || reference.starts_with(".\\")
        || reference.starts_with("..\\")
        || (reference.as_bytes().get(1) == Some(&b':')
            && reference.as_bytes()[0].is_ascii_alphabetic()
            && matches!(reference.as_bytes().get(2), Some(b'/' | b'\\')))
}
fn insert(
    found: &mut Vec<Discovered>,
    seen: &mut BTreeSet<String>,
    reference: &str,
    gallery: bool,
) {
    if (super::remote_reference(reference) || local_image(reference))
        && seen.insert(reference.to_owned())
    {
        found.push(Discovered {
            reference: reference.to_owned(),
            gallery,
        });
    }
}
fn local_image(reference: &str) -> bool {
    if reference.is_empty() {
        return false;
    }
    if reference
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("file:"))
    {
        return crate::files::normalize_media_ref(reference)
            .is_ok_and(|path| image_extension(&path));
    }
    #[cfg(windows)]
    let normalized = crate::files::normalize_media_ref(reference).ok();
    #[cfg(windows)]
    let reference = normalized.as_deref().unwrap_or(reference);
    if let Some(colon) = reference.find(':') {
        let bytes = reference.as_bytes();
        if colon != 1
            || !bytes[0].is_ascii_alphabetic()
            || !matches!(bytes.get(2), Some(b'/' | b'\\'))
        {
            return false;
        }
    }
    image_extension(reference)
}
fn image_extension(reference: &str) -> bool {
    let Some((_, extension)) = reference.rsplit_once('.') else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "apng"
    )
}
fn fence_line(line: &str) -> Option<(u8, usize, &str)> {
    let spaces = line.bytes().take_while(|b| *b == b' ').count();
    if spaces > 3 {
        return None;
    }
    let rest = &line[spaces..];
    let marker = *rest.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let count = rest.bytes().take_while(|b| *b == marker).count();
    (count >= 3).then_some((marker, count, &rest[count..]))
}
fn markdown(line: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = line.as_bytes();
    let mut p = start + 2;
    let mut nesting = 0;
    loop {
        match *bytes.get(p)? {
            b'\\' => p += 2,
            b'[' => {
                nesting += 1;
                p += 1;
            }
            b']' if nesting == 0 => break,
            b']' => {
                nesting -= 1;
                p += 1;
            }
            _ => p += 1,
        }
    }
    p += 1;
    if bytes.get(p) != Some(&b'(') {
        return None;
    }
    p += 1;
    while bytes.get(p).is_some_and(u8::is_ascii_whitespace) {
        p += 1;
    }
    let begin;
    let end;
    if bytes.get(p) == Some(&b'<') {
        p += 1;
        begin = p;
        while *bytes.get(p)? != b'>' {
            p += 1;
        }
        end = p;
        p += 1;
    } else {
        begin = p;
        let mut depth = 0;
        loop {
            match *bytes.get(p)? {
                b'\\' => p += 2,
                b'(' => {
                    depth += 1;
                    p += 1;
                }
                b')' if depth == 0 => break,
                b')' => {
                    depth -= 1;
                    p += 1;
                }
                c if c.is_ascii_whitespace() => break,
                _ => p += 1,
            }
        }
        end = p;
    }
    while bytes.get(p).is_some_and(u8::is_ascii_whitespace) {
        p += 1;
    }
    if let Some(quote @ (b'\'' | b'"')) = bytes.get(p).copied() {
        p += 1;
        while *bytes.get(p)? != quote {
            if bytes[p] == b'\\' {
                p += 1;
            }
            p += 1;
        }
        p += 1;
        while bytes.get(p).is_some_and(u8::is_ascii_whitespace) {
            p += 1;
        }
    }
    if bytes.get(p) != Some(&b')') {
        return None;
    }
    Some((line.get(begin..end)?, p + 1))
}
