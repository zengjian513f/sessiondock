//! Case folding for the search prefilter, and the whole-word boundary class.
//!
//! The matcher's case-insensitive mode is the regex crate's: a literal
//! character `c` matches exactly the characters of its Unicode *simple*
//! case-folding class (`regex-syntax` `ClassUnicode::case_fold_simple`,
//! CaseFolding.txt statuses C and S, closed under equivalence). `fold_char`
//! maps every member of a class to the same representative — the smallest
//! code point of the class — so for any characters `c`, `d`:
//! `c` matches `d` case-insensitively ⟹ `fold_char(c) == fold_char(d)`.
//! Hence a body that matches a needle (case-sensitively or not) contains
//! the folded needle in its folded text as a byte substring: the folded
//! copies are a sound candidate filter. Folding is not length-preserving
//! (`K` U+212A folds to `K`), which is why the prefilter answers only
//! "contains", never a position; positions come from the matcher on the
//! original text.
//!
//! The table is computed from the crate's own tables at first use (below
//! `FOLD_SCAN_END`; `tests::folding_matches_the_matcher_on_every_code_point`
//! proves nothing above it has a mapping and that every class agrees with
//! the matcher).

use std::sync::OnceLock;

use regex_syntax::hir::{Class, ClassUnicode, ClassUnicodeRange, HirKind};

/// No simple case folding exists at or above this code point (planes 2+
/// are ideographs, tags and unassigned); the guard test enumerates every
/// code point to keep this true across `regex-syntax` updates.
const FOLD_SCAN_END: u32 = 0x1FFFF;
const BLOCK: u32 = 256;

struct Table {
    /// `(code point, representative)` for every code point whose
    /// representative differs from itself, sorted by code point.
    map: Vec<(u32, u32)>,
    /// One bit per 256-code-point block: whether `map` has an entry in it.
    blocks: Vec<u64>,
}

/// The smallest code point of `c`'s simple case-folding class.
fn class_min(c: char) -> char {
    let mut class = ClassUnicode::new([ClassUnicodeRange::new(c, c)]);
    class.case_fold_simple();
    class
        .ranges()
        .first()
        .map_or(c, ClassUnicodeRange::start)
        .min(c)
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = Vec::new();
        let mut blocks = vec![0u64; (FOLD_SCAN_END / BLOCK / 64 + 1) as usize];
        for point in 0..=FOLD_SCAN_END {
            let Some(c) = char::from_u32(point) else {
                continue;
            };
            let min = class_min(c);
            if min != c {
                map.push((point, u32::from(min)));
                let block = point / BLOCK;
                blocks[(block / 64) as usize] |= 1 << (block % 64);
            }
        }
        Table { map, blocks }
    })
}

/// The representative of `c`'s simple case-folding class (see module doc).
pub fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_uppercase();
    }
    let point = u32::from(c);
    if point > FOLD_SCAN_END {
        return c;
    }
    let table = table();
    let block = point / BLOCK;
    if table.blocks[(block / 64) as usize] & (1 << (block % 64)) == 0 {
        return c;
    }
    match table.map.binary_search_by_key(&point, |(from, _)| *from) {
        Ok(index) => char::from_u32(table.map[index].1).unwrap_or(c),
        Err(_) => c,
    }
}

/// Append the folded UTF-8 of `text` to `out`.
pub fn fold_into(text: &str, out: &mut Vec<u8>) {
    out.reserve(text.len());
    let bytes = text.as_bytes();
    let mut rest = 0;
    // ASCII runs are folded byte-wise; anything else goes through the table.
    while rest < bytes.len() {
        let ascii_end = bytes[rest..]
            .iter()
            .position(|byte| !byte.is_ascii())
            .map_or(bytes.len(), |offset| rest + offset);
        out.extend(bytes[rest..ascii_end].iter().map(u8::to_ascii_uppercase));
        rest = ascii_end;
        if rest == bytes.len() {
            break;
        }
        let other_end = bytes[rest..]
            .iter()
            .position(u8::is_ascii)
            .map_or(bytes.len(), |offset| rest + offset);
        let mut buffer = [0u8; 4];
        for c in text[rest..other_end].chars() {
            out.extend_from_slice(fold_char(c).encode_utf8(&mut buffer).as_bytes());
        }
        rest = other_end;
    }
}

/// The folded UTF-8 of `text`.
pub fn fold(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    fold_into(text, &mut out);
    out
}

/// The whole-word boundary class `[\p{L}\p{N}_]`, exactly as the matcher
/// compiles it (same `regex-syntax` tables), as sorted inclusive ranges.
fn word_class() -> &'static [(char, char)] {
    static CLASS: OnceLock<Vec<(char, char)>> = OnceLock::new();
    CLASS.get_or_init(|| {
        let hir = regex_syntax::Parser::new()
            .parse(r"[\p{L}\p{N}_]")
            .expect("constant class");
        match hir.kind() {
            HirKind::Class(Class::Unicode(class)) => class
                .ranges()
                .iter()
                .map(|range| (range.start(), range.end()))
                .collect(),
            _ => unreachable!("a class parses to a class"),
        }
    })
}

/// Whether `c` is in `[\p{L}\p{N}_]`: a whole-word match may neither be
/// preceded nor followed by such a character.
pub fn is_word_char(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    let class = word_class();
    class
        .binary_search_by(|(start, end)| {
            if *end < c {
                std::cmp::Ordering::Less
            } else if *start > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class_of(c: char) -> Vec<char> {
        let mut class = ClassUnicode::new([ClassUnicodeRange::new(c, c)]);
        class.case_fold_simple();
        class
            .ranges()
            .iter()
            .flat_map(|range| u32::from(range.start())..=u32::from(range.end()))
            .filter_map(char::from_u32)
            .collect()
    }

    /// Every code point: the fold is the class minimum, every member of a
    /// class folds alike, and the matcher (`(?i)c`) accepts exactly the
    /// members of `c`'s class — so a case-insensitive match implies equal
    /// folds. Also proves `FOLD_SCAN_END`.
    #[test]
    fn folding_matches_the_matcher_on_every_code_point() {
        let mut classes = 0;
        let mut members = 0;
        let mut shrinking = Vec::new();
        for point in 0..=0x10FFFF {
            let Some(c) = char::from_u32(point) else {
                continue;
            };
            let class = class_of(c);
            let min = *class.iter().min().expect("the class contains c");
            assert_eq!(fold_char(c), min, "U+{point:04X}");
            if class.len() == 1 {
                continue;
            }
            assert!(
                point <= FOLD_SCAN_END,
                "U+{point:04X} has a case-folding class above FOLD_SCAN_END"
            );
            if min == c {
                classes += 1;
            }
            members += 1;
            for &d in &class {
                assert_eq!(
                    fold_char(d),
                    min,
                    "U+{:04X} in class of U+{point:04X}",
                    u32::from(d)
                );
                assert_eq!(
                    class_of(d),
                    class,
                    "class of U+{:04X} is not closed",
                    u32::from(d)
                );
            }
            if min.len_utf8() != c.len_utf8() {
                shrinking.push(c);
            }
        }
        assert!(
            classes > 1000 && members > 2 * classes,
            "{classes} classes, {members} members"
        );
        assert!(
            shrinking.contains(&'\u{212A}') && shrinking.contains(&'\u{17F}'),
            "Kelvin sign and long s fold to a shorter encoding: {shrinking:?}"
        );
        // Matcher agreement, on every non-trivial class: `(?i)c` matches d
        // exactly when they fold alike, and never matches across classes.
        let mut checked = 0;
        for &(point, _) in &table().map {
            let c = char::from_u32(point).unwrap();
            let class = class_of(c);
            let pattern = regex::RegexBuilder::new(&format!("^{}$", regex::escape(&c.to_string())))
                .case_insensitive(true)
                .build()
                .unwrap();
            for &d in &class {
                assert!(pattern.is_match(&d.to_string()), "(?i){c:?} vs {d:?}");
                checked += 1;
            }
            let fancy =
                fancy_regex::RegexBuilder::new(&format!("^{}$", regex::escape(&c.to_string())))
                    .case_insensitive(true)
                    .build()
                    .unwrap();
            for &d in &class {
                assert!(
                    fancy.is_match(&d.to_string()).unwrap(),
                    "fancy (?i){c:?} vs {d:?}"
                );
            }
            for d in [
                'a', 'z', '0', '猫', '\u{3B1}', '\u{1E9E}', 'ß', 'ſ', 'İ', 'ı',
            ] {
                assert_eq!(
                    pattern.is_match(&d.to_string()),
                    fold_char(d) == fold_char(c),
                    "(?i){c:?} vs {d:?}"
                );
            }
        }
        assert!(checked > 2000, "{checked}");
    }

    #[test]
    fn folding_examples_and_soundness_cases() {
        assert_eq!(fold("Hello, World_42"), b"HELLO, WORLD_42");
        assert_eq!(fold("ddp_guard"), fold("DDP_GUARD"));
        assert_eq!(fold("k"), fold("\u{212A}"), "Kelvin sign");
        assert_eq!(fold("s"), fold("\u{17F}"), "long s");
        assert_eq!(fold("ß"), fold("\u{1E9E}"), "capital sharp s");
        assert_eq!(fold("Σσς"), fold("σσσ"));
        assert_eq!(
            fold("İ"),
            "İ".as_bytes(),
            "dotted capital I is its own class"
        );
        assert_eq!(fold("ı"), "ı".as_bytes(), "dotless i is its own class");
        assert_ne!(fold("i"), fold("ı"));
        assert_eq!(fold("会话列表"), "会话列表".as_bytes());
        assert_eq!(fold("\x1b[31mA\x1b[0m"), b"\x1b[31MA\x1b[0M");
        let mixed = "abcΩ\u{2126}k\u{212A}猫";
        let mut out = b"pre".to_vec();
        fold_into(mixed, &mut out);
        assert_eq!(&out[..3], b"pre");
        assert_eq!(&out[3..], &fold(mixed)[..]);
        assert!(std::str::from_utf8(&fold(mixed)).is_ok());
    }

    #[test]
    fn word_class_matches_the_matchers_class_on_every_code_point() {
        let pattern = regex::Regex::new(r"^[\p{L}\p{N}_]$").unwrap();
        let fancy = fancy_regex::Regex::new(r"^[\p{L}\p{N}_]$").unwrap();
        let mut buffer = [0u8; 4];
        let mut words = 0;
        for point in 0..=0x10FFFF {
            let Some(c) = char::from_u32(point) else {
                continue;
            };
            let text = c.encode_utf8(&mut buffer);
            let expected = pattern.is_match(text);
            assert_eq!(is_word_char(c), expected, "U+{point:04X}");
            if point % 97 == 0 || expected != c.is_alphanumeric() {
                assert_eq!(
                    fancy.is_match(text).unwrap(),
                    expected,
                    "fancy U+{point:04X}"
                );
            }
            words += usize::from(expected);
        }
        assert!(words > 100_000, "{words}");
        assert!(is_word_char('_') && is_word_char('猫') && is_word_char('9'));
        assert!(!is_word_char('#') && !is_word_char(' ') && !is_word_char('\u{301}'));
    }
}
