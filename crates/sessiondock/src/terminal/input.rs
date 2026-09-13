//! Raw HTTP terminal input: named-key mapping and per-instance rate limiting.
//!
//! This is not reliable delivery. A successful request only means the host
//! acknowledged the PTY write; nothing observes whether the CLI consumed it.
//! The composer, outbox and native confirmation stay unavailable.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

/// Decoded input bytes per request (text bytes, or the mapped key bytes).
pub const MAX_INPUT_BYTES: usize = 16 * 1024;
/// Named keys per request; the byte bound still applies.
pub const MAX_KEYS: usize = 256;
/// Requests admitted per exact host instance within one sliding window.
pub const RATE_LIMIT: usize = 16;
pub const RATE_WINDOW: Duration = Duration::from_secs(1);
const MAX_RATE_ENTRIES: usize = 4096;

/// One named key accepted by the HTTP API, resolved to the key name the host's
/// `keys` operation expects. The host renders the final bytes itself so arrow
/// keys honour the application's DECCKM state; `bytes` is the byte length
/// either way (normal and application cursor sequences have equal length).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappedKey {
    pub host_name: &'static str,
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
/// `pageup`, `ctrl-c`, `^c`). Anything else is rejected: the host would
/// otherwise type an unknown name literally, which is never intended input.
pub fn map_key(raw: &str) -> Option<MappedKey> {
    if raw.is_empty() || raw.len() > 16 || !raw.is_ascii() {
        return None;
    }
    for (host_name, aliases, bytes) in NAMED {
        if raw == *host_name || aliases.iter().any(|alias| raw.eq_ignore_ascii_case(alias)) {
            return Some(MappedKey {
                host_name,
                bytes: bytes.len(),
            });
        }
    }
    // One ASCII letter or digit is typed literally, which is also what the
    // host does with it: the Codex question menu answers with `1`–`9` and a
    // command approval with its `y` / `p` mnemonic (WP-G question cards).
    if let [byte] = raw.as_bytes()
        && byte.is_ascii_alphanumeric()
        && let Some(index) = LITERAL.find(*byte as char)
    {
        return Some(MappedKey {
            host_name: &LITERAL[index..index + 1],
            bytes: 1,
        });
    }
    let letter = raw
        .strip_prefix("C-")
        .or_else(|| raw.strip_prefix("c-"))
        .or_else(|| raw.strip_prefix("ctrl-"))
        .or_else(|| raw.strip_prefix("Ctrl-"))
        .or_else(|| raw.strip_prefix("CTRL-"))
        .or_else(|| raw.strip_prefix('^'))?;
    let mut chars = letter.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if !c.is_ascii_alphabetic() {
        return None;
    }
    let index = (c.to_ascii_lowercase() as u8 - b'a') as usize;
    Some(MappedKey {
        host_name: CONTROL[index],
        bytes: 1,
    })
}

/// Map a whole request. Errors name the offending key without echoing text.
pub fn map_keys(raw: &[String]) -> Result<(Vec<&'static str>, usize), KeyError> {
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
    /// Index of the first unsupported key name.
    Unknown(usize),
}

/// Sliding-window admission per exact host instance. Windows for different
/// instances are independent; nothing is batched or shared across them. Idle
/// entries are dropped after one window, so the map is bounded by recently
/// active leases rather than by history.
#[derive(Default)]
pub struct InputRate {
    windows: BTreeMap<String, VecDeque<Duration>>,
}

impl InputRate {
    /// Admit one request at monotonic time `now`, or return the delay after
    /// which the oldest counted request leaves the window.
    pub fn admit(&mut self, key: &str, now: Duration) -> Result<(), Duration> {
        // A stamp counts while `stamp > now - window`; before one window has
        // elapsed since the clock origin nothing can have left it.
        let horizon = now.checked_sub(RATE_WINDOW);
        self.windows.retain(|_, stamps| {
            while let Some(horizon) = horizon
                && stamps.front().is_some_and(|stamp| *stamp <= horizon)
            {
                stamps.pop_front();
            }
            !stamps.is_empty()
        });
        if !self.windows.contains_key(key) && self.windows.len() >= MAX_RATE_ENTRIES {
            return Err(RATE_WINDOW);
        }
        let stamps = self.windows.entry(key.to_owned()).or_default();
        if stamps.len() >= RATE_LIMIT {
            let oldest = stamps.front().copied().unwrap_or(now);
            return Err((oldest + RATE_WINDOW)
                .saturating_sub(now)
                .max(Duration::from_millis(1)));
        }
        // Clamp: a caller sampling before another caller took the lock must
        // not reorder the window.
        let stamp = stamps.back().map_or(now, |last| now.max(*last));
        stamps.push_back(stamp);
        Ok(())
    }

    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.windows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_resolve_to_host_names_with_exact_byte_lengths() {
        let expect = |raw: &str, host: &'static str, bytes: usize| {
            assert_eq!(
                map_key(raw),
                Some(MappedKey {
                    host_name: host,
                    bytes
                }),
                "{raw}"
            );
        };
        expect("enter", "Enter", 1);
        expect("Enter", "Enter", 1);
        expect("ESC", "Escape", 1);
        expect("escape", "Escape", 1);
        expect("tab", "Tab", 1);
        expect("ctrl-c", "C-c", 1);
        expect("C-c", "C-c", 1);
        expect("^D", "C-d", 1);
        expect("Ctrl-Z", "C-z", 1);
        expect("up", "Up", 3);
        expect("Down", "Down", 3);
        expect("left", "Left", 3);
        expect("ArrowRight", "Right", 3);
        expect("pageup", "PPage", 4);
        expect("PPage", "PPage", 4);
        expect("page-down", "NPage", 4);
        expect("NPage", "NPage", 4);
        expect("home", "Home", 3);
        expect("End", "End", 3);
        expect("backspace", "BSpace", 1);
        expect("delete", "DC", 4);
        expect("F5", "F5", 5);
        // Host byte sequences the table accounts for, in normal cursor mode.
        for (host, _, bytes) in NAMED {
            assert_eq!(map_key(host).unwrap().bytes, bytes.len(), "{host}");
        }
    }

    #[test]
    fn unknown_or_literal_key_names_are_rejected() {
        // Single ASCII letters/digits are the one literal form accepted (see
        // `single_letters_and_digits_are_literal_keys`); everything else that
        // is not a host name or alias stays rejected.
        for raw in [
            "",
            "xx",
            "ab",
            "C-",
            "C-Escape",
            "C-Left",
            "M-x",
            "S-Up",
            "ctrl-1",
            "C-[",
            "C-@",
            "enter ",
            " enter",
            "Enter\n",
            "ping",
            "quit\r",
            "\x1b[A",
            "F13",
            "C-ab",
            "ｅｎｔｅｒ",
        ] {
            assert_eq!(map_key(raw), None, "{raw:?}");
        }
        let long = "e".repeat(17);
        assert_eq!(map_key(&long), None);
    }

    #[test]
    fn single_letters_and_digits_are_literal_keys() {
        for raw in ["1", "9", "y", "p", "Y", "0"] {
            let mapped = map_key(raw).unwrap_or_else(|| panic!("{raw:?}"));
            assert_eq!(mapped.host_name, raw);
            assert_eq!(mapped.bytes, 1);
        }
        for raw in ["yy", "-", " ", "é", "!", "10"] {
            assert_eq!(map_key(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn key_requests_are_bounded_and_index_the_first_unknown_name() {
        assert_eq!(map_keys(&[]), Err(KeyError::Empty));
        let keys: Vec<String> = ["Down", "Down", "enter"]
            .iter()
            .map(|k| k.to_string())
            .collect();
        assert_eq!(map_keys(&keys), Ok((vec!["Down", "Down", "Enter"], 7)));
        let keys: Vec<String> = ["Down", "typed", "enter"]
            .iter()
            .map(|k| k.to_string())
            .collect();
        assert_eq!(map_keys(&keys), Err(KeyError::Unknown(1)));
        let keys = vec!["enter".to_string(); MAX_KEYS + 1];
        assert_eq!(map_keys(&keys), Err(KeyError::TooMany));
        let keys = vec!["enter".to_string(); MAX_KEYS];
        assert_eq!(map_keys(&keys).unwrap().1, MAX_KEYS);
    }

    #[test]
    fn rate_window_is_per_instance_sliding_and_bounded() {
        let mut rate = InputRate::default();
        let ms = Duration::from_millis;
        for index in 0..RATE_LIMIT {
            assert_eq!(rate.admit("term/one", ms(index as u64 * 10)), Ok(()));
        }
        // The 17th within the same second is refused with the exact delay.
        assert_eq!(rate.admit("term/one", ms(500)), Err(ms(500)));
        assert_eq!(rate.admit("term/one", ms(999)), Err(ms(1)));
        // Another instance keeps its own window.
        assert_eq!(rate.admit("term/two", ms(999)), Ok(()));
        // Sliding: once the first stamp leaves the window one slot frees up.
        assert_eq!(rate.admit("term/one", ms(1000)), Ok(()));
        assert_eq!(rate.admit("term/one", ms(1005)), Err(ms(5)));
        assert_eq!(rate.admit("term/one", ms(1010)), Ok(()));
        // Idle entries are dropped after a full window.
        assert_eq!(rate.tracked(), 2);
        assert_eq!(rate.admit("term/three", ms(3000)), Ok(()));
        assert_eq!(rate.tracked(), 1);
        // A monotonic sample that arrives late does not move the window backwards.
        assert_eq!(rate.admit("term/three", ms(2500)), Ok(()));
        assert_eq!(rate.windows["term/three"].back(), Some(&ms(3000)));
    }

    #[test]
    fn rate_map_capacity_fails_closed_for_new_instances_only() {
        let mut rate = InputRate::default();
        let base = Duration::from_secs(10);
        for index in 0..MAX_RATE_ENTRIES {
            assert_eq!(rate.admit(&format!("term-{index}/x"), base), Ok(()));
        }
        assert_eq!(rate.admit("term-new/x", base), Err(RATE_WINDOW));
        assert_eq!(rate.admit("term-0/x", base), Ok(()));
        assert_eq!(rate.admit("term-new/x", base + RATE_WINDOW), Ok(()));
        assert_eq!(rate.tracked(), 1);
    }
}
