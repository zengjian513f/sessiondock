//! Split the renderer's escaped text and span markup into independently mountable
//! pieces. End offsets use UTF-16 so the browser can slice its original string.
use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Chunk {
    end: usize,
    html: String,
}

pub(super) fn split(html: &str) -> Option<Vec<Chunk>> {
    if html.len() < 32 * 1024 {
        return None;
    }
    let mut chunks = Vec::new();
    let mut open = Vec::<&str>::new();
    let mut part = String::new();
    let (mut end, mut tags, mut lines) = (0, 0, 0);
    let mut rest = html;
    while !rest.is_empty() {
        if rest.starts_with('<') {
            let (tag, next) = rest.split_at(rest.find('>')? + 1);
            if tag == "</span>" {
                open.pop()?;
            } else if tag.starts_with("<span ") {
                open.push(tag);
                tags += 1;
            } else {
                return None;
            }
            part.push_str(tag);
            rest = next;
            continue;
        }
        let (text, next) = rest.split_at(rest.find('<').unwrap_or(rest.len()));
        for line in text.split_inclusive('\n') {
            part.push_str(line);
            end += html_escape::decode_html_entities(line)
                .encode_utf16()
                .count();
            lines += usize::from(line.ends_with('\n'));
            // Whole line groups match the initial plain-text layout. Keep an
            // extra cut for dense single lines, where a line budget cannot help.
            if lines >= 48 || tags >= 256 {
                for _ in &open {
                    part.push_str("</span>");
                }
                chunks.push(Chunk {
                    end,
                    html: std::mem::take(&mut part),
                });
                for tag in &open {
                    part.push_str(tag);
                }
                tags = 0;
                lines = 0;
            }
        }
        rest = next;
    }
    if chunks.is_empty() || !open.is_empty() {
        return None;
    }
    if chunks.last()?.end < end {
        chunks.push(Chunk { end, html: part });
    }
    Some(chunks)
}
