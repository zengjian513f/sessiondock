//! Raw HTTP terminal input: named-key mapping and host protocol bounds.
//!
//! This is not reliable delivery. A successful request only means the host
//! acknowledged the PTY write; nothing observes whether the CLI consumed it.
//! The composer, outbox and native confirmation stay unavailable.

/// Decoded input bytes per request (text bytes, or the mapped key bytes).
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
/// Named keys per request, matching ptyhost guarded_v1; the byte bound still applies.
pub const MAX_KEYS: usize = 256;
/// One named key accepted by the HTTP API, resolved to the key name the host's
/// `keys` operation expects. The host renders the final bytes itself so arrow
/// keys honour the application's DECCKM state; `bytes` is the byte length
/// either way (normal and application cursor sequences have equal length).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappedKey {
    pub host_name: String,
    pub bytes: usize,
}

/// Host-side byte sequences for reference and accounting (normal cursor mode).
const NAMED: &[(&str, &[&str], &[u8])] = &[
    ("Enter", &["enter", "return", "cr"], b"\r"),
    ("Escape", &["escape", "esc"], b"\x1b"),
    ("Tab", &["tab"], b"\t"),
    ("BTab", &["btab", "shift-tab", "backtab"], b"\x1b[Z"),
    ("BSpace", &["bspace", "backspace"], b"\x7f"),
    ("Space", &["space"], b" "),
    ("DC", &["dc", "delete", "del"], b"\x1b[3~"),
    ("IC", &["ic", "insert", "ins"], b"\x1b[2~"),
    ("Home", &["home"], b"\x1b[H"),
    ("End", &["end"], b"\x1b[F"),
    (
        "PPage",
        &["ppage", "pageup", "page-up", "page_up", "pgup"],
        b"\x1b[5~",
    ),
    (
        "NPage",
        &[
            "npage",
            "pagedown",
            "page-down",
            "page_down",
            "pgdn",
            "pgdown",
        ],
        b"\x1b[6~",
    ),
    ("Up", &["up", "arrowup"], b"\x1b[A"),
    ("Down", &["down", "arrowdown"], b"\x1b[B"),
    ("Right", &["right", "arrowright"], b"\x1b[C"),
    ("Left", &["left", "arrowleft"], b"\x1b[D"),
    ("F1", &["f1"], b"\x1bOP"),
    ("F2", &["f2"], b"\x1bOQ"),
    ("F3", &["f3"], b"\x1bOR"),
    ("F4", &["f4"], b"\x1bOS"),
    ("F5", &["f5"], b"\x1b[15~"),
    ("F6", &["f6"], b"\x1b[17~"),
    ("F7", &["f7"], b"\x1b[18~"),
    ("F8", &["f8"], b"\x1b[19~"),
    ("F9", &["f9"], b"\x1b[20~"),
    ("F10", &["f10"], b"\x1b[21~"),
    ("F11", &["f11"], b"\x1b[23~"),
    ("F12", &["f12"], b"\x1b[24~"),
];

/// Single literal keys (see `map_key`), indexed so each maps to a `&'static str`.
static LITERAL: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

const CONTROL: [&str; 26] = [
    "C-a", "C-b", "C-c", "C-d", "C-e", "C-f", "C-g", "C-h", "C-i", "C-j", "C-k", "C-l", "C-m",
    "C-n", "C-o", "C-p", "C-q", "C-r", "C-s", "C-t", "C-u", "C-v", "C-w", "C-x", "C-y", "C-z",
];

/// Resolve one request key name. Accepts the host's own tmux-style names
/// exactly (`Enter`, `PPage`, `C-c`) and lower-case aliases (`enter`,
/// `pageup`, `ctrl-c`, `^c`). Every other nonempty host-valid key is forwarded
/// literally, matching ptyhost's `key_bytes` fallback.
pub fn map_key(raw: &str) -> Option<MappedKey> {
    if raw.is_empty() || raw.len() > 256 {
        return None;
    }
    for (host_name, aliases, bytes) in NAMED {
        if raw == *host_name || aliases.iter().any(|alias| raw.eq_ignore_ascii_case(alias)) {
            return Some(MappedKey {
                host_name: (*host_name).into(),
                bytes: bytes.len(),
            });
        }
    }
    // One ASCII letter or digit is typed literally, which is also what the
    // host does with it: the Codex question menu answers with `1`–`9` and a
    // command approval with its `y` / `p` mnemonic (question cards).
    if let [byte] = raw.as_bytes()
        && byte.is_ascii_alphanumeric()
        && let Some(index) = LITERAL.find(*byte as char)
    {
        return Some(MappedKey {
            host_name: LITERAL[index..index + 1].into(),
            bytes: 1,
        });
    }
    let letter = raw
        .strip_prefix("C-")
        .or_else(|| raw.strip_prefix("c-"))
        .or_else(|| raw.strip_prefix("ctrl-"))
        .or_else(|| raw.strip_prefix("Ctrl-"))
        .or_else(|| raw.strip_prefix("CTRL-"))
        .or_else(|| raw.strip_prefix('^'));
    let Some(letter) = letter else {
        return Some(MappedKey {
            host_name: raw.into(),
            bytes: raw.len(),
        });
    };
    let mut chars = letter.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Some(MappedKey {
            host_name: raw.into(),
            bytes: raw.len(),
        });
    };
    if !c.is_ascii_alphabetic() {
        return Some(MappedKey {
            host_name: raw.into(),
            bytes: raw.len(),
        });
    }
    let index = (c.to_ascii_lowercase() as u8 - b'a') as usize;
    Some(MappedKey {
        host_name: CONTROL[index].into(),
        bytes: 1,
    })
}

/// Map a whole request. Errors name the offending key without echoing text.
pub fn map_keys(raw: &[String]) -> Result<(Vec<String>, usize), KeyError> {
    if raw.is_empty() {
        return Err(KeyError::Empty);
    }
    if raw.len() > MAX_KEYS {
        return Err(KeyError::TooMany);
    }
    let mut names = Vec::with_capacity(raw.len());
    let mut bytes = 0usize;
    for (index, key) in raw.iter().enumerate() {
        let mapped = map_key(key).ok_or(KeyError::Unknown(index))?;
        names.push(mapped.host_name);
        bytes += mapped.bytes;
    }
    if bytes > MAX_INPUT_BYTES {
        return Err(KeyError::TooLarge);
    }
    Ok((names, bytes))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    Empty,
    TooMany,
    TooLarge,
    /// Index of the first empty or host-oversized key name.
    Unknown(usize),
}
