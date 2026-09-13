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
    let mut scan_budget = 4 * 1024 * 1024;
    // Native record limits normally enforce this already. Do not build an
    // unbounded secondary scanner for callers with arbitrary Values.
    if text.len() > 2 * 1024 * 1024 {
        return found;
    }
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
            if let Some((reference, end)) = markdown(line, start, &mut scan_budget) {
                spans.push((start, end));
                position = end;
                insert(&mut found, &mut seen, reference, false);
                if found.len() == 16 {
                    return found;
                }
            }
            if scan_budget == 0 {
                return found;
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
                    if found.len() == 16 {
                        return found;
                    }
                }
                start = b;
            }
        }
    }
    found
}
/// Python's `_RAW_PATH` only treats `~`, `/`, `./` and `../` prefixed tokens as
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
    if local_image(reference) && seen.insert(reference.to_owned()) {
        found.push(Discovered {
            reference: reference.to_owned(),
            gallery,
        });
    }
}
fn local_image(reference: &str) -> bool {
    if reference.is_empty()
        || reference.len() > 4096
        || reference.contains("![")
        || reference.contains("](")
        || reference.starts_with("//")
        || reference.starts_with("\\\\")
        || reference
            .chars()
            .any(|c| c.is_control() || matches!(c, '?' | '#' | '<' | '>' | '"' | '\''))
    {
        return false;
    }
    if reference
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("file:"))
    {
        return crate::files::normalize_media_ref(reference)
            .is_ok_and(|path| image_extension(&path));
    }
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
fn markdown<'a>(line: &'a str, start: usize, budget: &mut usize) -> Option<(&'a str, usize)> {
    let bytes = line.as_bytes();
    let mut p = start + 2;
    let mut nesting = 0;
    loop {
        *budget = budget.checked_sub(1)?;
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
            *budget = budget.checked_sub(1)?;
            p += 1;
        }
        end = p;
        p += 1;
    } else {
        begin = p;
        let mut depth = 0;
        loop {
            *budget = budget.checked_sub(1)?;
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
            *budget = budget.checked_sub(1)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn pairs(role: &str, text: &str) -> Vec<(String, bool)> {
        discover(&json!({"role":role,"text":text}))
            .into_iter()
            .map(|d| (d.reference, d.gallery))
            .collect()
    }
    #[test]
    fn markdown_spaces_title_and_raw_candidates() {
        assert_eq!(
            pairs(
                "assistant",
                "![image](<./my image.PNG> \"title\") and `/tmp/cat.jpg` C:\\img\\cat.webp"
            ),
            vec![
                ("./my image.PNG".into(), false),
                ("/tmp/cat.jpg".into(), true),
                ("C:\\img\\cat.webp".into(), true)
            ]
        );
        assert_eq!(
            pairs("tool", "![image](./out(1).avif) ./ignored.bmp"),
            vec![("./out(1).avif".into(), false)]
        );
        // Bare names in prose are text (Python `_RAW_PATH` needs ~ / ./ ../).
        assert!(pairs("user", "see shot.png and 图.jpg").is_empty());
        assert_eq!(
            pairs(
                "user",
                "see ./shot.png ~/home.jpg ../up.gif /abs/x.webp D:/d/x.bmp"
            ),
            vec![
                ("./shot.png".into(), true),
                ("~/home.jpg".into(), true),
                ("../up.gif".into(), true),
                ("/abs/x.webp".into(), true),
                ("D:/d/x.bmp".into(), true)
            ]
        );
    }
    #[test]
    fn remote_data_and_nonimage_sources_are_never_candidates() {
        assert!(pairs("user","![x](https://example.invalid/x.png) ![x](data:image/png;base64,AAAA) ![x](//example.invalid/x.jpg) file://remote.invalid/tmp/x.bmp https://example.invalid/x.gif?x=.png /tmp/x.svg").is_empty());
        assert!(
            pairs(
                "user",
                "\\\\host\\share\\image.png ./secret.png?token=abc ./secret.png#fragment"
            )
            .is_empty()
        );
    }
    #[test]
    fn local_file_url_is_validated_without_changing_reference() {
        assert_eq!(
            pairs("user", "![x](file:///tmp/my%20image.png) ~/local.jpg"),
            vec![
                ("file:///tmp/my%20image.png".into(), false),
                ("~/local.jpg".into(), true)
            ]
        );
        assert!(
            pairs(
                "user",
                "file:///tmp/a%00.png file://other.invalid/tmp/a.png"
            )
            .is_empty()
        );
    }
    #[test]
    fn fenced_code_is_not_scanned() {
        assert_eq!(
            pairs(
                "user",
                "./before.png\n```sh\n./secret.jpg\n~~~\n![a](hidden.webp)\n```\n./after.gif\n   ~~~~text\n./inside.avif\n~~~\n./inside2.bmp\n~~~~\n./last.apng"
            ),
            vec![
                ("./before.png".into(), true),
                ("./after.gif".into(), true),
                ("./last.apng".into(), true)
            ]
        );
    }
    #[test]
    fn role_scope_metadata_and_deduplication() {
        for role in [
            "user",
            "assistant",
            "command",
            "·subagent",
            "user·subagent",
            "assistant·subagent",
            "command·subagent",
        ] {
            assert_eq!(pairs(role, "./x.png ./x.png").len(), 1);
        }
        for role in ["tool", "status", "system", ""] {
            assert!(pairs(role, "./x.png").is_empty());
        }
        assert!(
            discover(
                &json!({"role":"assistant","content":"secret.png","metadata":{"text":"hidden.jpg"}})
            )
            .is_empty()
        );
        assert_eq!(
            pairs("user", "![x](./one.png) ./one.png"),
            vec![("./one.png".into(), false)]
        );
    }
    #[test]
    fn maximum_and_malformed_utf8_boundaries() {
        let text = (0..100)
            .map(|n| format!("./图{n}.png"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(pairs("user", &text).len(), 16);
        for text in [
            "![x](<未完成.png",
            "![x](\\图.png)",
            "![x](foo.png \"unterminated)",
            "![嵌[套]图](./猫.png)",
        ] {
            let _ = pairs("assistant", text);
        }
        assert_eq!(pairs("user", "![嵌[套]图](./猫.png)")[0].0, "./猫.png");
    }
}
