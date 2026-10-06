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

fn class_ranges(pattern: &str) -> Vec<(char, char)> {
    let hir = regex_syntax::Parser::new()
        .parse(pattern)
        .expect("constant class");
    match hir.kind() {
        HirKind::Class(Class::Unicode(class)) => class
            .ranges()
            .iter()
            .map(|range| (range.start(), range.end()))
            .collect(),
        _ => unreachable!("a class parses to a class"),
    }
}

fn in_ranges(class: &[(char, char)], c: char) -> bool {
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

/// The whole-word class `[\p{L}\p{N}_]`, exactly as the matcher compiles it
/// (same `regex-syntax` tables), as sorted inclusive ranges.
fn word_class() -> &'static [(char, char)] {
    static CLASS: OnceLock<Vec<(char, char)>> = OnceLock::new();
    CLASS.get_or_init(|| class_ranges(r"[\p{L}\p{N}_]"))
}

/// Han, kana, hangul and bopomofo letters. These form words among themselves
/// and break from every other letter, so `tag` matches `的tag`.
fn cjk_script(c: char) -> bool {
    if c.is_ascii() {
        return false;
    }
    static CLASS: OnceLock<Vec<(char, char)>> = OnceLock::new();
    let class = CLASS.get_or_init(|| {
        class_ranges(
            r"[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}\p{Script=Bopomofo}]",
        )
    });
    in_ranges(class, c)
}

/// Whether `c` is in `[\p{L}\p{N}_]`.
pub fn is_word_char(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    in_ranges(word_class(), c)
}

fn is_cjk_letter(c: char) -> bool {
    is_word_char(c) && cjk_script(c)
}

fn is_non_cjk_letter(c: char) -> bool {
    is_word_char(c) && c != '_' && !c.is_numeric() && !cjk_script(c)
}

fn script_split(a: char, b: char) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return false;
    }
    (is_cjk_letter(a) && is_non_cjk_letter(b)) || (is_non_cjk_letter(a) && is_cjk_letter(b))
}

/// Whether `neighbor` blocks a whole-word hit whose adjacent match character
/// is `edge` (`None` when the match is empty). A word character blocks,
/// except a CJK letter against a non-CJK letter: `的` does not swallow `tag`,
/// while `猫猫` still swallows `猫`, and digits or `_` still join.
pub fn word_edge_blocks(neighbor: char, edge: Option<char>) -> bool {
    if !is_word_char(neighbor) {
        return false;
    }
    !matches!(edge, Some(edge) if script_split(neighbor, edge))
}
